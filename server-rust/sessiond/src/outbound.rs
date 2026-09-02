//! # Bounded Outbound Transaction Queues & Coalescing (§20.2)
//!
//! Provides bounded, per-connection transaction streaming with scalar property coalescing
//! and lossless structural barriers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use srui_protocol::Transaction;
use srui_semantic_tree::Transaction as DomainTxn;

use crate::session::SessionError;

/// Default capacity for per-connection outbound transaction queues (§20.2).
pub const DEFAULT_OUTBOUND_QUEUE_CAPACITY: usize = 128;

/// Errors occurring when receiving transactions from an [`OutboundReceiver`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboundRecvError {
    /// The client queue overflowed its configured capacity; connection must detach for resync (§20.2).
    #[error("outbound queue overflow: {0}")]
    Lagged(String),
    /// The outbound transaction queue or session has closed cleanly.
    #[error("outbound queue closed")]
    Closed,
}

#[derive(Debug)]
struct SubscriberState {
    capacity: usize,
    max_ops: usize,
    max_frame_size: usize,
    items: VecDeque<Transaction>,
    peak_depth: usize,
    stale_reason: Option<String>,
    is_closed: bool,
}

impl SubscriberState {
    fn try_push_or_absorb(&mut self, tx: &Transaction) -> Result<bool, OutboundRecvError> {
        if self.is_closed {
            if let Some(reason) = &self.stale_reason {
                return Err(OutboundRecvError::Lagged(reason.clone()));
            }
            return Err(OutboundRecvError::Closed);
        }

        // Try absorbing into the unsent tail item if present
        if let Some(tail) = self.items.back_mut() {
            // Convert to domain transactions to leverage canonical domain absorption logic
            if let (Ok(mut domain_tail), Ok(domain_incoming)) = (
                DomainTxn::try_from(tail.clone()),
                DomainTxn::try_from(tx.clone()),
            ) {
                if domain_tail.try_absorb(&domain_incoming, self.max_ops, self.max_frame_size) {
                    *tail = (&domain_tail).into();
                    return Ok(false); // Absorbed in-place, queue length did not increase
                }
            }
        }

        // Cannot absorb: check bounded capacity
        if self.items.len() >= self.capacity {
            let reason = format!(
                "client queue exceeded bounded capacity of {} transactions without absorption",
                self.capacity
            );
            self.stale_reason = Some(reason.clone());
            self.is_closed = true;
            self.items.clear();
            return Err(OutboundRecvError::Lagged(reason));
        }

        self.items.push_back(tx.clone());
        if self.items.len() > self.peak_depth {
            self.peak_depth = self.items.len();
        }
        Ok(true) // Enqueued as new item
    }
}

/// A handle for receiving outbound transactions streamed to a connection.
#[derive(Debug)]
pub struct OutboundReceiver {
    notify_rx: mpsc::Receiver<()>,
    state: Arc<Mutex<SubscriberState>>,
}

impl OutboundReceiver {
    /// Asynchronously waits for the next committed transaction.
    ///
    /// This is the single, authoritative signal for queue lag: on overflow, it returns
    /// [`OutboundRecvError::Lagged`].
    pub async fn recv(&mut self) -> Result<Transaction, OutboundRecvError> {
        loop {
            {
                let mut guard = self.state.lock().unwrap();
                if let Some(reason) = &guard.stale_reason {
                    return Err(OutboundRecvError::Lagged(reason.clone()));
                }
                if let Some(item) = guard.items.pop_front() {
                    return Ok(item);
                }
                if guard.is_closed {
                    return Err(OutboundRecvError::Closed);
                }
            }

            match self.notify_rx.recv().await {
                Some(()) => {}
                None => {
                    let guard = self.state.lock().unwrap();
                    if let Some(reason) = &guard.stale_reason {
                        return Err(OutboundRecvError::Lagged(reason.clone()));
                    }
                    return Err(OutboundRecvError::Closed);
                }
            }
        }
    }

    /// Non-blocking synchronous poll for the next transaction.
    pub fn try_recv(&mut self) -> Result<Option<Transaction>, OutboundRecvError> {
        let mut guard = self.state.lock().unwrap();
        if let Some(reason) = &guard.stale_reason {
            return Err(OutboundRecvError::Lagged(reason.clone()));
        }
        if let Some(item) = guard.items.pop_front() {
            let _ = self.notify_rx.try_recv();
            return Ok(Some(item));
        }
        if guard.is_closed {
            return Err(OutboundRecvError::Closed);
        }
        Ok(None)
    }

    /// Returns `true` if the underlying queue has closed.
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().is_closed
    }
}

#[derive(Debug)]
struct Subscriber {
    client_instance_id: Vec<u8>,
    notify_tx: mpsc::Sender<()>,
    state: Arc<Mutex<SubscriberState>>,
}

/// Central distribution hub for outbound transaction queues.
#[derive(Debug)]
pub(crate) struct OutboundHub {
    subscribers: Mutex<Vec<Subscriber>>,
    stale_clients: Mutex<HashSet<Vec<u8>>>,
    historical_peak_depths: Mutex<HashMap<Vec<u8>, usize>>,
    is_closed: AtomicBool,
}

impl OutboundHub {
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            stale_clients: Mutex::new(HashSet::new()),
            historical_peak_depths: Mutex::new(HashMap::new()),
            is_closed: AtomicBool::new(false),
        }
    }

    pub fn subscribe(
        &self,
        client_instance_id: Vec<u8>,
        capacity: usize,
        max_ops: usize,
        max_frame_size: usize,
    ) -> Result<OutboundReceiver, SessionError> {
        if client_instance_id.is_empty() {
            return Err(SessionError::InvalidInput(
                "client_instance_id must not be empty for outbound subscription".to_string(),
            ));
        }
        if capacity == 0 {
            return Err(SessionError::InvalidConfiguration(
                "outbound queue capacity must be positive".to_string(),
            ));
        }
        if self.is_closed.load(Ordering::Relaxed) {
            return Err(SessionError::OutboundClosed);
        }

        let (notify_tx, notify_rx) = mpsc::channel(capacity);
        let state = Arc::new(Mutex::new(SubscriberState {
            capacity,
            max_ops,
            max_frame_size,
            items: VecDeque::with_capacity(capacity),
            peak_depth: 0,
            stale_reason: None,
            is_closed: false,
        }));

        let sub = Subscriber {
            client_instance_id,
            notify_tx,
            state: Arc::clone(&state),
        };

        self.subscribers.lock().unwrap().push(sub);

        Ok(OutboundReceiver { notify_rx, state })
    }

    pub fn publish(&self, tx: &Transaction) {
        if self.is_closed.load(Ordering::Relaxed) {
            return;
        }

        let mut subs = self.subscribers.lock().unwrap();
        let mut stale_marked = Vec::new();

        subs.retain(|sub| {
            if sub.notify_tx.is_closed() {
                return false;
            }

            let mut guard = sub.state.lock().unwrap();
            match guard.try_push_or_absorb(tx) {
                Ok(true) => {
                    let _ = sub.notify_tx.try_send(());
                    true
                }
                Ok(false) => true,
                Err(OutboundRecvError::Lagged(_)) => {
                    stale_marked.push((sub.client_instance_id.clone(), guard.peak_depth));
                    false
                }
                Err(OutboundRecvError::Closed) => false,
            }
        });

        if !stale_marked.is_empty() {
            let mut stale_guard = self.stale_clients.lock().unwrap();
            let mut peaks = self.historical_peak_depths.lock().unwrap();
            for (id, peak) in stale_marked {
                stale_guard.insert(id.clone());
                peaks
                    .entry(id)
                    .and_modify(|p| *p = (*p).max(peak))
                    .or_insert(peak);
            }
        }
    }

    pub fn is_client_stale(&self, client_instance_id: &[u8]) -> bool {
        self.stale_clients
            .lock()
            .unwrap()
            .contains(client_instance_id)
    }

    pub fn clear_stale_client(&self, client_instance_id: &[u8]) {
        self.stale_clients
            .lock()
            .unwrap()
            .remove(client_instance_id);
    }

    #[cfg(test)]
    pub fn peak_depth_for_client(&self, client_instance_id: &[u8]) -> usize {
        let subs = self.subscribers.lock().unwrap();
        let mut peak = self
            .historical_peak_depths
            .lock()
            .unwrap()
            .get(client_instance_id)
            .copied()
            .unwrap_or(0);
        for sub in subs.iter() {
            if sub.client_instance_id == client_instance_id {
                let guard = sub.state.lock().unwrap();
                peak = peak.max(guard.peak_depth);
            }
        }
        peak
    }

    #[cfg(test)]
    pub fn max_peak_depth(&self) -> usize {
        let subs = self.subscribers.lock().unwrap();
        let mut max = self
            .historical_peak_depths
            .lock()
            .unwrap()
            .values()
            .copied()
            .max()
            .unwrap_or(0);
        for sub in subs.iter() {
            let guard = sub.state.lock().unwrap();
            max = max.max(guard.peak_depth);
        }
        max
    }

    #[cfg(test)]
    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::Relaxed)
    }

    pub fn close(&self) {
        self.is_closed.store(true, Ordering::Relaxed);
        let mut subs = self.subscribers.lock().unwrap();
        for sub in subs.drain(..) {
            let mut guard = sub.state.lock().unwrap();
            guard.is_closed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_protocol::{
        operation::Op, value::Value as WireValInner, CreateNodeOp, NodeRecord, Operation as WireOp,
        PropertyRef as WirePropRef, SetPropertyOp, Value as WireValue,
    };

    fn make_set_prop_op(node_id: u64, prop_id: u32, val: &str) -> WireOp {
        WireOp {
            op: Some(Op::SetProperty(SetPropertyOp {
                node_id,
                property: Some(WirePropRef {
                    namespace_id: 1,
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
        let hub = OutboundHub::new();
        let mut rx = hub
            .subscribe(vec![1], 4, 10, 1024 * 1024)
            .expect("subscribe");

        let tx1 = make_scalar_tx(0, 1, 10, 1, "v1");
        hub.publish(&tx1);
        assert_eq!(hub.peak_depth_for_client(&[1]), 1);

        let tx2 = make_scalar_tx(1, 2, 10, 1, "v2");
        hub.publish(&tx2);
        assert_eq!(hub.peak_depth_for_client(&[1]), 1);

        let tx3 = make_scalar_tx(2, 5, 10, 1, "v5");
        hub.publish(&tx3);
        assert_eq!(hub.peak_depth_for_client(&[1]), 1);

        let merged = rx.try_recv().expect("poll").expect("merged tx");
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
        let hub = OutboundHub::new();
        let mut rx = hub
            .subscribe(vec![1], 4, 10, 1024 * 1024)
            .expect("subscribe");

        let tx1 = make_scalar_tx(0, 1, 10, 1, "v1");
        hub.publish(&tx1);

        let tx_barrier = make_create_node_tx(1, 2, 20);
        hub.publish(&tx_barrier);
        assert_eq!(hub.peak_depth_for_client(&[1]), 2);

        let tx3 = make_scalar_tx(2, 3, 10, 1, "v3");
        hub.publish(&tx3);
        assert_eq!(hub.peak_depth_for_client(&[1]), 3);

        let tx4 = make_scalar_tx(3, 4, 10, 1, "v4");
        hub.publish(&tx4);
        assert_eq!(hub.peak_depth_for_client(&[1]), 3);

        let p1 = rx.try_recv().unwrap().unwrap();
        assert_eq!(p1.new_revision, 1);
        let p2 = rx.try_recv().unwrap().unwrap();
        assert_eq!(p2.new_revision, 2);
        let p3 = rx.try_recv().unwrap().unwrap();
        assert_eq!(p3.new_revision, 4);
    }

    #[test]
    fn test_overflow_discards_backlog_and_marks_stale() {
        let hub = OutboundHub::new();
        let mut rx = hub
            .subscribe(vec![7, 7], 2, 10, 1024 * 1024)
            .expect("subscribe");

        let b1 = make_create_node_tx(0, 1, 1);
        let b2 = make_create_node_tx(1, 2, 2);
        let b3 = make_create_node_tx(2, 3, 3);

        hub.publish(&b1);
        hub.publish(&b2);
        assert_eq!(hub.peak_depth_for_client(&[7, 7]), 2);
        assert_eq!(hub.max_peak_depth(), 2);

        hub.publish(&b3);
        assert!(hub.is_client_stale(&[7, 7]));

        assert!(matches!(rx.try_recv(), Err(OutboundRecvError::Lagged(_))));
    }
}
