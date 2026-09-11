//! # Bounded Outbound Transaction Queues & Coalescing (§12.1, §19.2, §20.2, §20.4)
//!
//! Provides bounded, per-connection transaction streaming with scalar property coalescing
//! and lossless structural barriers, plus class-aware resource metadata/chunk delivery
//! selected by [`scheduler`] (§14, §19.2).
//!
//! What this queue emits is a *delivery* stream: either a committed transaction verbatim, or a
//! coalesced scalar delta standing in for a run of them (§12.1). Neither the merge policy nor its
//! revision spans belong to the authoritative transaction model, so the merge itself lives in
//! [`coalesce`] rather than in `srui-semantic-tree`.
//!
//! Control, input, and terminal frames never enter the UI coalescing path. Resource metadata
//! and chunks are both `resource` traffic and are generated lazily, one frame per selection.

mod coalesce;
mod receiver;
mod resource;
mod scheduler;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use srui_protocol::{ResourceChunk, ResourceMetadata, Transaction};
use srui_resources::ResourceEntry;
use srui_semantic_tree::{ResourceHash, Transaction as DomainTxn};

use crate::session::{lock_or_recover, SessionError};

pub use receiver::OutboundReceiver;
pub use resource::ResourceOutboundFrame;
pub use scheduler::{
    logical_class_for_server_envelope, server_envelope_matches_class, LogicalChannelClass,
    LogicalChannelScheduler, SERVICE_CYCLE,
};

/// Default capacity for per-connection outbound transaction queues (§20.2).
pub const DEFAULT_OUTBOUND_QUEUE_CAPACITY: usize = 128;

/// Maximum number of overflowed `client_instance_id`s tracked for forced resync (§20.2).
///
/// `client_instance_id` is client-supplied, so the marker table is bounded on the same terms as
/// the journal retention window (§18.1): the oldest marker is evicted once the bound is reached.
/// An evicted client that later resumes falls back to journal-gap evaluation.
pub const MAX_TRACKED_STALE_CLIENTS: usize = 1024;

/// One outbound delivery unit: UI transaction or a single resource frame (§14, §19.2).
#[derive(Debug, Clone, PartialEq)]
pub enum OutboundItem {
    /// Committed (or coalesced) semantic transaction.
    Transaction(Transaction),
    /// Resource announcement for a content-addressed payload.
    ResourceMetadata(ResourceMetadata),
    /// One contiguous chunk of a resource payload.
    ResourceChunk(ResourceChunk),
}

impl OutboundItem {
    /// Returns the enclosed transaction when this item is UI traffic.
    #[must_use]
    pub fn as_transaction(&self) -> Option<&Transaction> {
        match self {
            Self::Transaction(tx) => Some(tx),
            _ => None,
        }
    }

    /// Consumes the item, returning the transaction when present.
    #[must_use]
    pub fn into_transaction(self) -> Option<Transaction> {
        match self {
            Self::Transaction(tx) => Some(tx),
            _ => None,
        }
    }

    /// Logical class for this outbound unit (§19.2).
    #[must_use]
    pub fn logical_class(&self) -> LogicalChannelClass {
        match self {
            Self::Transaction(_) => LogicalChannelClass::Ui,
            Self::ResourceMetadata(_) | Self::ResourceChunk(_) => LogicalChannelClass::Resource,
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Per-thread count of wire -> domain transaction conversions, asserted by the publish-cost tests.
    static WIRE_TO_DOMAIN_CONVERSIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Single funnel for wire -> domain transaction conversion so its per-publish cost is measurable.
fn to_domain(tx: &Transaction) -> Option<DomainTxn> {
    #[cfg(test)]
    WIRE_TO_DOMAIN_CONVERSIONS.with(|count| count.set(count.get() + 1));
    DomainTxn::try_from(tx.clone()).ok()
}

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
    /// Negotiated per-connection encoded resource ceiling from ClientHello (§15, §26).
    max_resource_size: u64,
    items: VecDeque<Transaction>,
    /// Domain form of `items.back()`, the only item absorption can still merge into.
    ///
    /// Cached so a publish never re-decodes an already-queued transaction: the wire -> domain
    /// conversion and the merge both run under the subscriber lock (§20.2).
    tail_domain: Option<DomainTxn>,
    /// Low-priority resource transfer cursor for this connection (§14, §19.2).
    resources: resource::ResourceTransferQueue,
    peak_depth: usize,
    stale_reason: Option<String>,
    is_closed: bool,
}

impl SubscriberState {
    /// Returns `true` if an unsent tail exists that absorption could still merge into.
    fn has_unsent_tail(&self) -> bool {
        !self.is_closed && !self.items.is_empty()
    }

    /// Pops the next queued transaction, keeping the cached tail consistent with the queue.
    fn pop_transaction(&mut self) -> Option<Transaction> {
        let item = self.items.pop_front()?;
        if self.items.is_empty() {
            self.tail_domain = None;
        }
        Some(item)
    }

    /// Enqueues `entry` when it fits this subscriber's negotiated resource ceiling (§15, §26).
    fn maybe_enqueue_resource(&mut self, entry: ResourceEntry) {
        if entry.encoded_length > self.max_resource_size {
            tracing::debug!(
                hash = %entry.hash,
                encoded_length = entry.encoded_length,
                max_resource_size = self.max_resource_size,
                "skipping resource transfer above negotiated client ceiling"
            );
            return;
        }
        self.resources.enqueue(entry);
    }

    fn class_ready(&self, class: LogicalChannelClass) -> bool {
        match class {
            LogicalChannelClass::Ui => !self.items.is_empty(),
            LogicalChannelClass::Resource => self.resources.has_work(),
            LogicalChannelClass::Control
            | LogicalChannelClass::Input
            | LogicalChannelClass::TerminalHigh
            | LogicalChannelClass::TerminalNormal => false,
        }
    }

    fn has_scheduled_work(&self) -> bool {
        self.class_ready(LogicalChannelClass::Ui) || self.class_ready(LogicalChannelClass::Resource)
    }

    /// Pops one frame of `class`. Control, input, and terminal never share the UI coalescing queue.
    fn pop_class(&mut self, class: LogicalChannelClass) -> Option<OutboundItem> {
        match class {
            LogicalChannelClass::Ui => self.pop_transaction().map(OutboundItem::Transaction),
            LogicalChannelClass::Resource => match self.resources.pop_frame() {
                Some(ResourceOutboundFrame::Metadata(meta)) => {
                    Some(OutboundItem::ResourceMetadata(meta))
                }
                Some(ResourceOutboundFrame::Chunk(chunk)) => {
                    Some(OutboundItem::ResourceChunk(chunk))
                }
                None => None,
            },
            LogicalChannelClass::Control
            | LogicalChannelClass::Input
            | LogicalChannelClass::TerminalHigh
            | LogicalChannelClass::TerminalNormal => None,
        }
    }

    fn clear(&mut self) {
        self.items.clear();
        self.tail_domain = None;
        self.resources.clear();
    }

    /// Enqueues `tx`, or merges it into the unsent tail when `incoming_domain` allows coalescing.
    ///
    /// `incoming_domain` is the caller's single decode of `tx`, shared across all subscribers.
    fn try_push_or_absorb(
        &mut self,
        tx: &Transaction,
        incoming_domain: Option<&DomainTxn>,
    ) -> Result<bool, OutboundRecvError> {
        if self.is_closed {
            if let Some(reason) = &self.stale_reason {
                return Err(OutboundRecvError::Lagged(reason.clone()));
            }
            return Err(OutboundRecvError::Closed);
        }

        // Try absorbing into the unsent tail item if present, using the canonical domain
        // absorption logic against the cached domain form of that tail.
        if let Some(incoming) = incoming_domain {
            if self.tail_domain.is_none() {
                // The tail was queued while no decode was in hand (empty -> non-empty queue);
                // decode it once and keep it for every later publish.
                if let Some(tail) = self.items.back() {
                    self.tail_domain = to_domain(tail);
                }
            }
            if let (Some(tail), Some(tail_domain)) =
                (self.items.back_mut(), self.tail_domain.as_mut())
            {
                if coalesce::try_absorb(tail_domain, incoming, self.max_ops, self.max_frame_size) {
                    *tail = (&*tail_domain).into();
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
            self.clear();
            return Err(OutboundRecvError::Lagged(reason));
        }

        self.items.push_back(tx.clone());
        self.tail_domain = incoming_domain.cloned();
        if self.items.len() > self.peak_depth {
            self.peak_depth = self.items.len();
        }
        Ok(true) // Enqueued as new item
    }
}

/// Bounded record of `client_instance_id`s whose queue overflowed and that must resync (§20.2).
///
/// Bounded because `client_instance_id` is client-supplied: a reconnect loop minting a fresh id per
/// attempt would otherwise grow this table for the lifetime of the daemon. Eviction is oldest-first,
/// matching the journal's bounded retention policy (§18.1).
#[derive(Debug, Default)]
struct StaleClientRegistry {
    /// Insertion order of `entries` keys, used for oldest-first eviction.
    order: VecDeque<Vec<u8>>,
    /// `client_instance_id` -> highest queue depth observed before overflow.
    entries: HashMap<Vec<u8>, usize>,
}

impl StaleClientRegistry {
    fn mark(&mut self, client_instance_id: Vec<u8>, peak_depth: usize) {
        if let Some(peak) = self.entries.get_mut(&client_instance_id) {
            *peak = (*peak).max(peak_depth);
            return;
        }

        self.entries.insert(client_instance_id.clone(), peak_depth);
        self.order.push_back(client_instance_id);
        while self.order.len() > MAX_TRACKED_STALE_CLIENTS {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
            }
        }
    }

    fn contains(&self, client_instance_id: &[u8]) -> bool {
        self.entries.contains_key(client_instance_id)
    }

    /// Client-supplied bytes retained by this table, i.e. the identifiers themselves.
    ///
    /// The entry cap bounds how many identifiers are kept, not how large each one is; this is the
    /// quantity a retention invariant can assert against (§20.2, §26).
    fn retained_key_bytes(&self) -> usize {
        self.entries.keys().map(Vec::len).sum()
    }

    fn clear_client(&mut self, client_instance_id: &[u8]) {
        if self.entries.remove(client_instance_id).is_some() {
            // Bounded by MAX_TRACKED_STALE_CLIENTS, so the linear scan is bounded too.
            self.order.retain(|id| id != client_instance_id);
        }
    }

    #[cfg(test)]
    fn peak_depth(&self, client_instance_id: &[u8]) -> usize {
        self.entries.get(client_instance_id).copied().unwrap_or(0)
    }

    #[cfg(test)]
    fn max_peak_depth(&self) -> usize {
        self.entries.values().copied().max().unwrap_or(0)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug)]
struct Subscriber {
    client_instance_id: Vec<u8>,
    notify_tx: mpsc::Sender<()>,
    state: Arc<Mutex<SubscriberState>>,
    disconnect: CancellationToken,
}

/// Central distribution hub for outbound transaction queues.
///
/// # Lock Order
///
/// `subscribers -> SubscriberState -> stale_clients`. Handshake bootstrap enters this hub while
/// holding the session lock, so the full order is `SessionInner -> subscribers -> SubscriberState
/// -> stale_clients`; nothing may acquire them in the opposite direction. A subscriber's
/// [`CancellationToken`] is cancelled only after its `SubscriberState` guard is released, both to
/// publish the terminal state before waking a connection and to keep an inline waker from
/// re-entering that lock.
#[derive(Debug)]
pub(crate) struct OutboundHub {
    subscribers: Mutex<Vec<Subscriber>>,
    stale_clients: Mutex<StaleClientRegistry>,
    is_closed: AtomicBool,
}

impl OutboundHub {
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            stale_clients: Mutex::new(StaleClientRegistry::default()),
            is_closed: AtomicBool::new(false),
        }
    }

    /// Registers a bounded outbound queue for one connection (§20.2).
    ///
    /// `client_instance_id` may be empty: it is a proto3 `bytes` field with no non-empty
    /// requirement, and an omitted id must not cost the client its connection.
    pub fn subscribe(
        &self,
        client_instance_id: Vec<u8>,
        capacity: usize,
        max_ops: usize,
        max_frame_size: usize,
        max_resource_size: u64,
    ) -> Result<OutboundReceiver, SessionError> {
        if capacity == 0 {
            return Err(SessionError::InvalidConfiguration(
                "outbound queue capacity must be positive".to_string(),
            ));
        }
        if self.is_closed.load(Ordering::Relaxed) {
            return Err(SessionError::OutboundClosed);
        }

        let (notify_tx, notify_rx) = mpsc::channel(capacity);
        let disconnect = CancellationToken::new();
        let state = Arc::new(Mutex::new(SubscriberState {
            capacity,
            max_ops,
            max_frame_size,
            max_resource_size,
            items: VecDeque::with_capacity(capacity),
            tail_domain: None,
            resources: resource::ResourceTransferQueue::default(),
            peak_depth: 0,
            stale_reason: None,
            is_closed: false,
        }));

        let sub = Subscriber {
            client_instance_id,
            notify_tx,
            state: Arc::clone(&state),
            disconnect: disconnect.clone(),
        };

        let mut subs = lock_or_recover(&self.subscribers);
        // Reap connections that ended since the last publish, so an idle session does not retain a
        // subscriber entry per connect/disconnect cycle (§20.2).
        subs.retain(|sub| !sub.notify_tx.is_closed() && !lock_or_recover(&sub.state).is_closed);
        subs.push(sub);
        drop(subs);

        Ok(OutboundReceiver::new(notify_rx, state, disconnect))
    }

    pub fn publish(&self, tx: &Transaction) {
        if self.is_closed.load(Ordering::Relaxed) {
            return;
        }

        let mut subs = lock_or_recover(&self.subscribers);
        let mut stale_marked = Vec::new();
        // Decoded at most once per commit and shared by every subscriber: this conversion runs
        // under the subscriber locks, so doing it per subscriber would stall a runtime worker on
        // behalf of one slow client (§20.2).
        let mut incoming_domain: Option<Option<DomainTxn>> = None;

        subs.retain(|sub| {
            if sub.notify_tx.is_closed() {
                return false;
            }

            let (outcome, peak_depth) = {
                let mut guard = lock_or_recover(&sub.state);
                let incoming = if guard.has_unsent_tail() {
                    incoming_domain
                        .get_or_insert_with(|| to_domain(tx))
                        .as_ref()
                } else {
                    None
                };
                let outcome = guard.try_push_or_absorb(tx, incoming);
                (outcome, guard.peak_depth)
            };

            match outcome {
                Ok(true) => {
                    let _ = sub.notify_tx.try_send(());
                    true
                }
                Ok(false) => true,
                Err(OutboundRecvError::Lagged(_)) => {
                    // Cancelled after the state guard is released: the terminal state must be
                    // readable by the woken connection, and an inline waker must not re-enter
                    // the subscriber lock.
                    stale_marked.push((sub.client_instance_id.clone(), peak_depth));
                    sub.disconnect.cancel();
                    false
                }
                Err(OutboundRecvError::Closed) => {
                    sub.disconnect.cancel();
                    false
                }
            }
        });
        drop(subs);

        if !stale_marked.is_empty() {
            let mut stale_guard = lock_or_recover(&self.stale_clients);
            for (id, peak) in stale_marked {
                stale_guard.mark(id, peak);
            }
        }
    }

    /// Enqueues a newly inserted resource for delivery on every live subscriber (§14, §19.2).
    ///
    /// Deduplicated republication should not call this: only freshly inserted CAS content needs
    /// transfer work.
    pub fn publish_resource(&self, entry: &ResourceEntry) {
        if self.is_closed.load(Ordering::Relaxed) {
            return;
        }

        let mut subs = lock_or_recover(&self.subscribers);
        subs.retain(|sub| {
            if sub.notify_tx.is_closed() {
                return false;
            }
            let closed = {
                let mut guard = lock_or_recover(&sub.state);
                if guard.is_closed {
                    true
                } else {
                    guard.maybe_enqueue_resource(entry.clone());
                    let _ = sub.notify_tx.try_send(());
                    false
                }
            };
            if closed {
                sub.disconnect.cancel();
                false
            } else {
                true
            }
        });
    }

    /// Seeds one subscriber with retained resources during handshake bootstrap (§14, §18, §20.2).
    pub fn seed_resources(&self, receiver: &OutboundReceiver, entries: &[ResourceEntry]) {
        self.seed_resources_excluding(receiver, entries, &HashSet::new());
    }

    /// Seeds retained resources except hashes the client has already verified (§14, §18).
    ///
    /// Called while the session lock is held, immediately after [`OutboundHub::subscribe`], so a
    /// resource published between snapshot creation and live subscription cannot be missed.
    pub fn seed_resources_excluding(
        &self,
        receiver: &OutboundReceiver,
        entries: &[ResourceEntry],
        known_hashes: &HashSet<ResourceHash>,
    ) {
        if entries.is_empty() {
            return;
        }
        // Lock order: subscribers -> SubscriberState (never state first).
        let subs = lock_or_recover(&self.subscribers);
        let Some(sub) = subs
            .iter()
            .find(|sub| Arc::ptr_eq(&sub.state, &receiver.state))
        else {
            return;
        };
        let mut enqueued = false;
        {
            let mut guard = lock_or_recover(&sub.state);
            if guard.is_closed {
                return;
            }
            for entry in entries {
                if known_hashes.contains(&entry.hash) {
                    continue;
                }
                guard.maybe_enqueue_resource(entry.clone());
                enqueued = true;
            }
        }
        if enqueued {
            let _ = sub.notify_tx.try_send(());
        }
    }

    /// Client-supplied bytes retained by the stale-client registry (§20.2, §26).
    pub fn retained_stale_client_bytes(&self) -> usize {
        lock_or_recover(&self.stale_clients).retained_key_bytes()
    }

    pub fn is_client_stale(&self, client_instance_id: &[u8]) -> bool {
        lock_or_recover(&self.stale_clients).contains(client_instance_id)
    }

    pub fn clear_stale_client(&self, client_instance_id: &[u8]) {
        lock_or_recover(&self.stale_clients).clear_client(client_instance_id);
    }

    #[cfg(test)]
    pub fn peak_depth_for_client(&self, client_instance_id: &[u8]) -> usize {
        let subs = lock_or_recover(&self.subscribers);
        let mut peak = lock_or_recover(&self.stale_clients).peak_depth(client_instance_id);
        for sub in subs.iter() {
            if sub.client_instance_id == client_instance_id {
                let guard = lock_or_recover(&sub.state);
                peak = peak.max(guard.peak_depth);
            }
        }
        peak
    }

    #[cfg(test)]
    pub fn subscriber_count(&self) -> usize {
        lock_or_recover(&self.subscribers).len()
    }

    #[cfg(test)]
    pub fn tracked_stale_client_count(&self) -> usize {
        lock_or_recover(&self.stale_clients).len()
    }

    #[cfg(test)]
    pub fn max_peak_depth(&self) -> usize {
        let subs = lock_or_recover(&self.subscribers);
        let mut max = lock_or_recover(&self.stale_clients).max_peak_depth();
        for sub in subs.iter() {
            let guard = lock_or_recover(&sub.state);
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
        let mut subs = lock_or_recover(&self.subscribers);
        for sub in subs.drain(..) {
            // Publish the terminal state *before* waking the connection: a connection woken by the
            // token reads `termination()`, and an unmarked subscriber reads as a lagged queue, so
            // a clean shutdown would be reported as LaggedResyncRequired (§20.2). Already-queued
            // transactions stay drainable; only the receiver's own drop discards them.
            lock_or_recover(&sub.state).is_closed = true;
            sub.disconnect.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_protocol::{
        operation::Op, srui_message, value::Value as WireValInner, CreateNodeOp, NodeRecord,
        Operation as WireOp, PropertyRef as WirePropRef, SetPropertyOp, SruiMessage,
        Value as WireValue,
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
            .subscribe(vec![1], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
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

        let merged = rx
            .try_recv_class(LogicalChannelClass::Ui)
            .expect("poll")
            .expect("merged tx")
            .into_transaction()
            .expect("transaction item");
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
            .subscribe(vec![1], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
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

        let p1 = rx
            .try_recv_class(LogicalChannelClass::Ui)
            .unwrap()
            .unwrap()
            .into_transaction()
            .unwrap();
        assert_eq!(p1.new_revision, 1);
        let p2 = rx
            .try_recv_class(LogicalChannelClass::Ui)
            .unwrap()
            .unwrap()
            .into_transaction()
            .unwrap();
        assert_eq!(p2.new_revision, 2);
        let p3 = rx
            .try_recv_class(LogicalChannelClass::Ui)
            .unwrap()
            .unwrap()
            .into_transaction()
            .unwrap();
        assert_eq!(p3.new_revision, 4);
    }

    #[test]
    fn test_coalescing_accounts_for_enclosing_message_frame_size() {
        let tx1 = make_scalar_tx(0, 1, 10, 1, "first-value");
        let tx2 = make_scalar_tx(1, 2, 10, 2, "second-value");
        let merged = Transaction {
            base_revision: 0,
            new_revision: 2,
            priority: 1,
            operations: vec![tx1.operations[0].clone(), tx2.operations[0].clone()],
        };
        let merged_envelope = SruiMessage {
            msg: Some(srui_message::Msg::Transaction(merged.clone())),
        };
        let framed =
            srui_protocol::encode_framed(&merged_envelope).expect("encode merged envelope fixture");
        let merged_envelope_size = (0..=framed.len())
            .find(|&limit| srui_protocol::encode_framed_with_limit(&merged_envelope, limit).is_ok())
            .expect("find encoded envelope size");
        let max_frame_size = merged_envelope_size - 1;

        assert!(
            srui_protocol::encode_framed_with_limit(&merged, max_frame_size).is_ok(),
            "fixture must fit the bare transaction under the limit"
        );
        for tx in [&tx1, &tx2] {
            let envelope = SruiMessage {
                msg: Some(srui_message::Msg::Transaction(tx.clone())),
            };
            assert!(
                srui_protocol::encode_framed_with_limit(&envelope, max_frame_size).is_ok(),
                "each unmerged transaction must remain independently sendable"
            );
        }

        let hub = OutboundHub::new();
        let mut rx = hub
            .subscribe(vec![8, 8], 4, 10, max_frame_size, 50 * 1024 * 1024)
            .expect("subscribe");

        hub.publish(&tx1);
        hub.publish(&tx2);

        assert_eq!(
            hub.peak_depth_for_client(&[8, 8]),
            2,
            "coalescing must not create an SruiMessage larger than the frame limit"
        );
        assert_eq!(
            rx.try_recv_class(LogicalChannelClass::Ui)
                .unwrap()
                .unwrap()
                .into_transaction()
                .unwrap(),
            tx1
        );
        assert_eq!(
            rx.try_recv_class(LogicalChannelClass::Ui)
                .unwrap()
                .unwrap()
                .into_transaction()
                .unwrap(),
            tx2
        );
    }

    #[test]
    fn test_overflow_discards_backlog_and_marks_stale() {
        let hub = OutboundHub::new();
        let mut rx = hub
            .subscribe(vec![7, 7], 2, 10, 1024 * 1024, 50 * 1024 * 1024)
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
        assert!(
            rx.disconnect_token().is_cancelled(),
            "overflow must wake a connection blocked in framed_write.send"
        );

        assert!(matches!(
            rx.try_recv_class(LogicalChannelClass::Ui),
            Err(OutboundRecvError::Lagged(_))
        ));
        assert!(matches!(
            rx.termination(),
            Some(OutboundRecvError::Lagged(_))
        ));
    }

    /// A connection woken by the disconnect token reads [`OutboundReceiver::termination`] to decide
    /// between a clean unwind and `LaggedResyncRequired`, so the terminal state must be published
    /// *before* the token fires (§20.2).
    #[test]
    fn test_close_marks_subscriber_closed_before_cancelling_disconnect_token() {
        use std::future::Future;
        use std::task::{Context, Poll, Wake, Waker};

        struct ObservingWaker {
            state: Arc<Mutex<SubscriberState>>,
            observed_closed: Mutex<Option<bool>>,
        }

        impl Wake for ObservingWaker {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }

            fn wake_by_ref(self: &Arc<Self>) {
                let closed = self.state.lock().unwrap().is_closed;
                *self.observed_closed.lock().unwrap() = Some(closed);
            }
        }

        let hub = OutboundHub::new();
        let rx = hub
            .subscribe(vec![3], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
            .expect("subscribe");

        let observer = Arc::new(ObservingWaker {
            state: Arc::clone(&rx.state),
            observed_closed: Mutex::new(None),
        });
        let waker = Waker::from(Arc::clone(&observer));
        let mut cx = Context::from_waker(&waker);
        let token = rx.disconnect_token().clone();
        let mut cancelled = Box::pin(token.cancelled());
        assert!(matches!(cancelled.as_mut().poll(&mut cx), Poll::Pending));

        hub.close();

        assert_eq!(
            *observer.observed_closed.lock().unwrap(),
            Some(true),
            "disconnect token fired before the subscriber was marked closed; \
             a clean shutdown would be reported as LaggedResyncRequired"
        );
    }

    /// `client_instance_id` is client-supplied, so overflow bookkeeping must not grow without bound
    /// across reconnect loops that mint a fresh id every attempt (§20.2).
    #[test]
    fn test_stale_client_tracking_is_bounded() {
        let hub = OutboundHub::new();
        let client_count = MAX_TRACKED_STALE_CLIENTS + 16;
        let mut receivers = Vec::with_capacity(client_count);
        for i in 0..client_count {
            receivers.push(
                hub.subscribe(
                    i.to_be_bytes().to_vec(),
                    1,
                    10,
                    1024 * 1024,
                    50 * 1024 * 1024,
                )
                .expect("subscribe"),
            );
        }

        hub.publish(&make_create_node_tx(0, 1, 1));
        hub.publish(&make_create_node_tx(1, 2, 2));

        assert!(
            hub.tracked_stale_client_count() <= MAX_TRACKED_STALE_CLIENTS,
            "stale-client bookkeeping must stay bounded, tracked {} entries for {} clients",
            hub.tracked_stale_client_count(),
            client_count
        );
    }

    /// Publishing must convert the incoming wire transaction to its domain form once per commit,
    /// not once per backed-up subscriber while holding the subscriber and state locks (§20.2).
    #[test]
    fn test_publish_converts_incoming_transaction_once() {
        let hub = OutboundHub::new();
        let mut receivers = Vec::new();
        for i in 0..8u8 {
            receivers.push(
                hub.subscribe(vec![i], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
                    .expect("subscribe"),
            );
        }

        // Every subscriber now holds an unsent tail whose domain form is cached, so the absorb
        // path runs for each of them on the publish under measurement.
        hub.publish(&make_create_node_tx(0, 1, 1));
        hub.publish(&make_scalar_tx(1, 2, 10, 1, "v2"));

        WIRE_TO_DOMAIN_CONVERSIONS.with(|count| count.set(0));
        hub.publish(&make_scalar_tx(2, 3, 10, 1, "v3"));
        let conversions = WIRE_TO_DOMAIN_CONVERSIONS.with(std::cell::Cell::get);

        assert_eq!(
            conversions, 1,
            "one publish must decode the incoming transaction once, not once per subscriber"
        );
    }

    /// A poisoned lock must fail the connection that panicked, not every later publish and
    /// subscribe on the daemon (same policy as `session::lock_or_recover`).
    #[test]
    fn test_hub_survives_poisoned_locks() {
        let hub = OutboundHub::new();
        let mut rx = hub
            .subscribe(vec![5], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
            .expect("subscribe");

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = hub.subscribers.lock().unwrap();
            panic!("deliberate poison of the subscribers lock");
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = hub.stale_clients.lock().unwrap();
            panic!("deliberate poison of the stale-client lock");
        }));

        let tx = make_create_node_tx(0, 1, 1);
        hub.publish(&tx);
        assert_eq!(
            rx.try_recv_class(LogicalChannelClass::Ui)
                .expect("poll")
                .expect("queued tx")
                .into_transaction()
                .unwrap(),
            tx
        );
        assert!(!hub.is_client_stale(&[5]));
        let _second = hub
            .subscribe(vec![6], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
            .expect("subscribe must still work after a poisoned lock");
    }

    /// Disconnected subscribers must not accumulate on a session that goes idle between
    /// connect/disconnect cycles (§20.2).
    #[test]
    fn test_dropped_receivers_do_not_accumulate() {
        let hub = OutboundHub::new();
        for _ in 0..5 {
            let rx = hub
                .subscribe(vec![1], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
                .expect("subscribe");
            drop(rx);
        }

        let _live = hub
            .subscribe(vec![2], 4, 10, 1024 * 1024, 50 * 1024 * 1024)
            .expect("subscribe");

        assert_eq!(
            hub.subscriber_count(),
            1,
            "dropped receivers must be reaped rather than retained until the next publish"
        );
    }

    /// `ClientHello.client_instance_id` is a proto3 `bytes` field with no non-empty requirement,
    /// so an omitted id must still receive transactions rather than lose the connection.
    /// The stale registry caps entries, not identifier size. Marking far more clients than the cap
    /// allows, each with a maximal identifier, must plateau in *bytes* — the quantity a count
    /// assertion cannot see (§20.2, §26).
    #[test]
    fn test_stale_registry_retained_bytes_plateau_under_distinct_ids() {
        use crate::session::MAX_CLIENT_INSTANCE_ID_BYTES;

        let mut registry = StaleClientRegistry::default();
        let budget = MAX_TRACKED_STALE_CLIENTS * MAX_CLIENT_INSTANCE_ID_BYTES;

        let mut at_cap = None;
        for nonce in 0..(MAX_TRACKED_STALE_CLIENTS as u64 * 4) {
            let mut id = nonce.to_be_bytes().to_vec();
            id.resize(MAX_CLIENT_INSTANCE_ID_BYTES, 0xAB);
            registry.mark(id, 1);

            let retained = registry.retained_key_bytes();
            assert!(
                retained <= budget,
                "after {} marks: retained {retained} bytes exceeds the {budget} byte budget",
                nonce + 1
            );
            if registry.len() == MAX_TRACKED_STALE_CLIENTS {
                match at_cap {
                    None => at_cap = Some(retained),
                    Some(previous) => assert_eq!(
                        previous, retained,
                        "retained bytes must stop growing once the entry cap is reached"
                    ),
                }
            }
        }

        assert_eq!(
            at_cap,
            Some(budget),
            "the sequence must saturate the table, or the bound is untested"
        );
    }

    #[test]
    fn test_subscribe_accepts_empty_client_instance_id() {
        let hub = OutboundHub::new();
        let mut rx = hub
            .subscribe(Vec::new(), 4, 10, 1024 * 1024, 50 * 1024 * 1024)
            .expect("empty client_instance_id is the proto3 default, not a handshake failure");

        let tx = make_create_node_tx(0, 1, 1);
        hub.publish(&tx);
        assert_eq!(
            rx.try_recv_class(LogicalChannelClass::Ui)
                .expect("poll")
                .expect("queued tx")
                .into_transaction()
                .unwrap(),
            tx
        );
    }
}
