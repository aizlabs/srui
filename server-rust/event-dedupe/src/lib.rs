//! # SRUI Event Deduplication
//!
//! Provides sliding-window deduplication for incoming client events (§18.2, §18.3, §20.2, §21, §32.4).
//! Enforces bounded memory limits per client to prevent memory exhaustion.
//!
//! The window doubles as the bounded **event result cache** of Appendix B:
//! `(client_instance_id, event_id) -> {status, revision_after_effect}`. Re-delivery of a
//! settled event returns its prior outcome, while `last_processed_event_seq` reports only the
//! highest **contiguous** settled sequence. Like a TCP cumulative ACK, it never crosses a gap.

use std::collections::{BTreeSet, HashMap, VecDeque};

use bytes::Bytes;
use srui_protocol::Event;
use thiserror::Error;

/// Default maximum number of recent event IDs retained per client instance (4096 events).
pub const DEFAULT_MAX_DEDUPE_ENTRIES: usize = 4096;

/// Default maximum number of distinct client instances retained per session (§26, §27).
///
/// App. B bounds the receive/result window *per client instance*; without a bound on the number of
/// instances the map is still unbounded, because `client_instance_id` is peer-chosen and the
/// reference client mints a fresh one on every launch. Windows survive detach on purpose (§18.2),
/// so the only reclamation available is least-recently-used eviction.
pub const DEFAULT_MAX_CLIENT_WINDOWS: usize = 256;

/// Settled outcome of one client event, retained for replay answering (§18.2, App. B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventOutcomeRecord {
    /// Whether the event was accepted and dispatched, or refused by validation.
    pub accepted: bool,
    /// Store revision observed after the event's side effects committed (App. B).
    pub revision_after_effect: u64,
    /// Validation refusal returned by a replayed rejected event.
    pub reject_reason: String,
}

/// A new event violated the bounded, contiguous per-client receive window.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventSequenceError {
    #[error("event_id must be non-empty for sequence-aware delivery")]
    MissingEventId,

    #[error("event replay changed event_seq from {expected_event_seq} to {received_event_seq}")]
    ReplaySequenceMismatch {
        expected_event_seq: u64,
        received_event_seq: u64,
    },

    #[error("event_seq {event_seq} is already assigned to another event_id")]
    SequenceAlreadyAssigned { event_seq: u64 },

    #[error(
        "event_seq {event_seq} is outside receive window {first_acceptable_seq}..={last_acceptable_seq}"
    )]
    OutsideReceiveWindow {
        event_seq: u64,
        first_acceptable_seq: u64,
        last_acceptable_seq: u64,
    },

    #[error("event receive window is full while earlier sequences remain unsettled")]
    ReceiveWindowFull,

    #[error(
        "event dedupe capacity exhausted: all {limit} retained client instances have unsettled events"
    )]
    ClientWindowCapacityExhausted { limit: usize },
}

/// Result of recording an event against a client's dedupe window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The event id had not been seen: the caller must now validate and dispatch it.
    Fresh { last_processed_event_seq: u64 },
    /// The event is currently being dispatched by another connection. This is not terminal and
    /// must not be acknowledged as processed or duplicate.
    Pending { last_processed_event_seq: u64 },
    /// The event id was already settled; `prior` is the cached outcome to echo (§18.2).
    Duplicate {
        prior: EventOutcomeRecord,
        last_processed_event_seq: u64,
    },
}

/// Bounded sliding-window event deduplicator, bounded both per client instance and in the number
/// of retained client instances (§18.2, §26, §27, App. B).
#[derive(Debug, Clone)]
pub struct EventDeduplicator {
    max_entries_per_client: usize,
    max_client_windows: usize,
    clients: HashMap<Bytes, ClientDedupeWindow>,
    /// Client instances in least-recently-used order; front is the next eviction candidate.
    client_order: VecDeque<Bytes>,
    /// Contiguous frontiers of evicted clients, so a returning client is not restarted at zero.
    ///
    /// Bounded by `max_client_windows` on the same terms as `clients` (§26): the entries are one
    /// `u64` each, and the oldest is dropped once the bound is reached. A client whose frontier
    /// has also aged out is genuinely indistinguishable from a new one.
    evicted_frontiers: HashMap<Bytes, u64>,
    /// Retained frontiers in insertion order; front is the next to be dropped.
    frontier_order: VecDeque<Bytes>,
}

/// State of one admitted `event_id` inside a client's receive window (§18.2).
///
/// `InFlight` means the sequence slot is owned but no terminal outcome exists yet, so the entry
/// can neither be acknowledged nor evicted. Only `Settled` carries the wire-visible outcome a
/// replay is answered from.
#[derive(Debug, Clone)]
enum EventRecord {
    InFlight {
        event_seq: u64,
    },
    Settled {
        event_seq: u64,
        outcome: EventOutcomeRecord,
    },
}

impl EventRecord {
    fn event_seq(&self) -> u64 {
        match self {
            Self::InFlight { event_seq } | Self::Settled { event_seq, .. } => *event_seq,
        }
    }

    fn is_evictable(&self, last_contiguous_processed_seq: u64) -> bool {
        matches!(
            self,
            Self::Settled { event_seq, .. }
                if *event_seq == 0 || *event_seq <= last_contiguous_processed_seq
        )
    }
}

#[derive(Debug, Clone)]
struct ClientDedupeWindow {
    seen_ids: HashMap<Bytes, EventRecord>,
    ids_by_seq: HashMap<u64, Bytes>,
    order: VecDeque<Bytes>,
    last_contiguous_processed_seq: u64,
    settled_out_of_order: BTreeSet<u64>,
}

impl ClientDedupeWindow {
    fn new() -> Self {
        Self {
            seen_ids: HashMap::new(),
            ids_by_seq: HashMap::new(),
            order: VecDeque::new(),
            last_contiguous_processed_seq: 0,
            settled_out_of_order: BTreeSet::new(),
        }
    }

    fn contains(&self, event_id: &[u8]) -> bool {
        self.seen_ids.contains_key(event_id)
    }

    /// Whether any admitted event is still awaiting a terminal outcome (§18.2, App. B).
    ///
    /// An `IN_FLIGHT` entry is never evictable — dropping it would let the client's retry be
    /// admitted as fresh and re-run a side effect that is currently executing.
    fn has_in_flight(&self) -> bool {
        self.seen_ids
            .values()
            .any(|record| matches!(record, EventRecord::InFlight { .. }))
    }

    /// Whether this window holds state that its contiguous frontier alone cannot reconstruct.
    ///
    /// Eviction keeps only `last_contiguous_processed_seq` (see
    /// [`EventDeduplicator::make_room_for_new_client`]). That is enough to refuse anything at or
    /// below the frontier, but a sequence settled *above* a gap is invisible to it: the client
    /// would retry it, the rebuilt window would admit it as fresh, and the side effect would run
    /// twice. Both conditions therefore pin the window in place (§18.2).
    fn is_pinned(&self) -> bool {
        self.has_in_flight() || !self.settled_out_of_order.is_empty()
    }

    fn admit(
        &mut self,
        event_id: &[u8],
        event_seq: u64,
        max_entries: usize,
    ) -> Result<RecordOutcome, EventSequenceError> {
        if let Some(record) = self.seen_ids.get(event_id) {
            let expected_event_seq = record.event_seq();
            if event_seq != expected_event_seq {
                return Err(EventSequenceError::ReplaySequenceMismatch {
                    expected_event_seq,
                    received_event_seq: event_seq,
                });
            }
            return Ok(match record {
                EventRecord::InFlight { .. } => RecordOutcome::Pending {
                    last_processed_event_seq: self.last_contiguous_processed_seq,
                },
                EventRecord::Settled { outcome, .. } => RecordOutcome::Duplicate {
                    prior: outcome.clone(),
                    last_processed_event_seq: self.last_contiguous_processed_seq,
                },
            });
        }

        // A sequence still owned by another `event_id` is a client bug, not a window overflow:
        // reporting it as such keeps the two protocol violations distinguishable.
        if self.ids_by_seq.contains_key(&event_seq) {
            return Err(EventSequenceError::SequenceAlreadyAssigned { event_seq });
        }

        // Sequence 0 is never allocated, so the frontier doubles as "nothing settled yet".
        // Saturating arithmetic keeps a frontier near `u64::MAX` from wrapping the window back
        // around onto an already-settled sequence.
        let first_acceptable_seq = self.last_contiguous_processed_seq.saturating_add(1);
        let window_width = u64::try_from(max_entries).unwrap_or(u64::MAX);
        let last_acceptable_seq = self
            .last_contiguous_processed_seq
            .saturating_add(window_width);
        if event_seq == 0
            || event_seq <= self.last_contiguous_processed_seq
            || event_seq > last_acceptable_seq
        {
            return Err(EventSequenceError::OutsideReceiveWindow {
                event_seq,
                first_acceptable_seq,
                last_acceptable_seq,
            });
        }

        // Only settled results at or below the contiguous frontier are evictable, and only in
        // FIFO order: an in-flight admission or an out-of-order settlement is still load-bearing.
        while self.seen_ids.len() >= max_entries {
            if !self.evict_oldest_settled() {
                return Err(EventSequenceError::ReceiveWindowFull);
            }
        }

        let id = Bytes::copy_from_slice(event_id);
        self.order.push_back(id.clone());
        self.ids_by_seq.insert(event_seq, id.clone());
        self.seen_ids
            .insert(id, EventRecord::InFlight { event_seq });

        Ok(RecordOutcome::Fresh {
            last_processed_event_seq: self.last_contiguous_processed_seq,
        })
    }

    fn insert_legacy_settled(
        &mut self,
        event_id: &[u8],
        outcome: EventOutcomeRecord,
        max_entries: usize,
    ) -> bool {
        if self.seen_ids.contains_key(event_id) {
            return false;
        }
        while self.seen_ids.len() >= max_entries {
            if !self.evict_oldest_settled() {
                return false;
            }
        }

        let id = Bytes::copy_from_slice(event_id);
        self.order.push_back(id.clone());
        self.seen_ids.insert(
            id,
            EventRecord::Settled {
                event_seq: 0,
                outcome,
            },
        );
        true
    }

    fn evict_oldest_settled(&mut self) -> bool {
        let Some(position) = self.order.iter().position(|id| {
            self.seen_ids
                .get(id)
                .is_some_and(|record| record.is_evictable(self.last_contiguous_processed_seq))
        }) else {
            return false;
        };
        let Some(id) = self.order.remove(position) else {
            return false;
        };
        if let Some(record) = self.seen_ids.remove(&id) {
            let event_seq = record.event_seq();
            if event_seq != 0 {
                self.ids_by_seq.remove(&event_seq);
                self.settled_out_of_order.remove(&event_seq);
            }
        }
        true
    }

    fn settle(&mut self, event_id: &[u8], outcome: EventOutcomeRecord) -> u64 {
        let event_seq = {
            let Some(slot) = self.seen_ids.get_mut(event_id) else {
                return self.last_contiguous_processed_seq;
            };
            match slot {
                EventRecord::InFlight { event_seq } => {
                    let event_seq = *event_seq;
                    *slot = EventRecord::Settled { event_seq, outcome };
                    event_seq
                }
                EventRecord::Settled { .. } => return self.last_contiguous_processed_seq,
            }
        };

        self.note_settled_sequence(event_seq);
        self.last_contiguous_processed_seq
    }

    fn note_settled_sequence(&mut self, event_seq: u64) {
        if event_seq == 0 || event_seq <= self.last_contiguous_processed_seq {
            return;
        }

        if event_seq == self.last_contiguous_processed_seq.saturating_add(1) {
            self.last_contiguous_processed_seq = event_seq;
            while let Some(next) = self.last_contiguous_processed_seq.checked_add(1) {
                if !self.settled_out_of_order.remove(&next) {
                    break;
                }
                self.last_contiguous_processed_seq = next;
            }
        } else {
            self.settled_out_of_order.insert(event_seq);
        }
    }

    fn abandon(&mut self, event_id: &[u8]) {
        let event_seq = self.seen_ids.get(event_id).and_then(|record| match record {
            EventRecord::InFlight { event_seq } => Some(*event_seq),
            EventRecord::Settled { .. } => None,
        });
        let Some(event_seq) = event_seq else {
            return;
        };

        self.seen_ids.remove(event_id);
        self.ids_by_seq.remove(&event_seq);
        self.order.retain(|id| id.as_ref() != event_id);
    }
}

impl Default for EventDeduplicator {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DEDUPE_ENTRIES)
    }
}

impl EventDeduplicator {
    /// Creates a new `EventDeduplicator` with the specified bounded capacity per client and the
    /// default bound on retained client instances.
    #[must_use]
    pub fn new(max_entries_per_client: usize) -> Self {
        Self::with_limits(max_entries_per_client, DEFAULT_MAX_CLIENT_WINDOWS)
    }

    /// Creates a new `EventDeduplicator` with explicit per-client and per-session bounds (§26).
    #[must_use]
    pub fn with_limits(max_entries_per_client: usize, max_client_windows: usize) -> Self {
        Self {
            max_entries_per_client: max_entries_per_client.max(1),
            max_client_windows: max_client_windows.max(1),
            clients: HashMap::new(),
            client_order: VecDeque::new(),
            evicted_frontiers: HashMap::new(),
            frontier_order: VecDeque::new(),
        }
    }

    /// Number of client instances currently retained.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Moves an existing client instance to the most-recently-used end of the eviction order.
    fn touch_client(&mut self, client_instance_id: &[u8]) {
        let Some(position) = self
            .client_order
            .iter()
            .position(|id| id.as_ref() == client_instance_id)
        else {
            return;
        };
        if position + 1 == self.client_order.len() {
            return;
        }
        if let Some(entry) = self.client_order.remove(position) {
            self.client_order.push_back(entry);
        }
    }

    /// Frees a slot for a previously unseen client instance (§26, §27).
    ///
    /// Evicts least-recently-used windows that carry no `IN_FLIGHT` admission. If every retained
    /// window is mid-dispatch the new instance is refused instead, because §18.2 forbids evicting
    /// an in-flight event to admit another.
    fn make_room_for_new_client(&mut self) -> Result<(), EventSequenceError> {
        while self.clients.len() >= self.max_client_windows {
            let candidate = self.client_order.iter().position(|id| {
                self.clients
                    .get(id)
                    .is_some_and(|window| !window.is_pinned())
            });
            let Some(position) = candidate else {
                return Err(EventSequenceError::ClientWindowCapacityExhausted {
                    limit: self.max_client_windows,
                });
            };
            let Some(evicted) = self.client_order.remove(position) else {
                break;
            };
            let window = self.clients.remove(&evicted);
            let frontier = window
                .as_ref()
                .map_or(0, |w| w.last_contiguous_processed_seq);

            // The frontier outlives the window it came from. Dropping it too would restart the
            // client at sequence zero, so its next event — numbered from where it actually left
            // off — lands past `max_entries_per_client` and is refused as `OutsideReceiveWindow`.
            // That is unrecoverable rather than merely lossy: `RESUME_OK` would report
            // `last_processed_event_seq = 0`, the client would keep its own counter, and every
            // reconnect would reproduce the same rejection (§18.2).
            self.retain_frontier(evicted.clone(), frontier);

            // §26 requires an eviction that can change observable behaviour to be reported rather
            // than dropped silently: a later replay from this instance is re-run instead of being
            // answered from the result cache.
            tracing::warn!(
                client_instance_id = ?evicted.as_ref(),
                retained_entries = window.map_or(0, |w| w.seen_ids.len()),
                retained_frontier = frontier,
                limit = self.max_client_windows,
                "evicted least-recently-used event dedupe window (§18.2, §26)"
            );
        }
        Ok(())
    }

    /// Records an evicted client's contiguous frontier, bounded on the same terms as the windows.
    ///
    /// A frontier is one `u64` plus its client id, so retaining `max_client_windows` of them costs
    /// a fraction of the windows themselves and keeps total growth bounded (§26). A frontier of 0
    /// carries no information and is not worth a slot.
    fn retain_frontier(&mut self, client_instance_id: Bytes, frontier: u64) {
        if frontier == 0 {
            return;
        }
        if self
            .evicted_frontiers
            .insert(client_instance_id.clone(), frontier)
            .is_none()
        {
            self.frontier_order.push_back(client_instance_id);
        }
        while self.frontier_order.len() > self.max_client_windows {
            let Some(oldest) = self.frontier_order.pop_front() else {
                break;
            };
            self.evicted_frontiers.remove(&oldest);
        }
    }

    /// Rebuilds a window for a client whose previous one was evicted, restoring its frontier.
    fn window_for_new_client(&mut self, client_instance_id: &[u8]) -> ClientDedupeWindow {
        let mut window = ClientDedupeWindow::new();
        if let Some(frontier) = self.evicted_frontiers.remove(client_instance_id) {
            self.frontier_order
                .retain(|id| id.as_ref() != client_instance_id);
            window.last_contiguous_processed_seq = frontier;
        }
        window
    }

    /// Checks whether an event has already been processed without recording it.
    #[must_use]
    pub fn is_duplicate(&self, client_instance_id: &[u8], event_id: &[u8]) -> bool {
        if event_id.is_empty() {
            return false;
        }
        self.clients
            .get(client_instance_id)
            .is_some_and(|window| window.contains(event_id))
    }

    /// Records an event ID using the legacy ID-only API.
    ///
    /// Sequence-aware delivery should use [`Self::admit_event`] and [`Self::settle_event`].
    pub fn record(&mut self, client_instance_id: &[u8], event_id: &[u8]) -> bool {
        if event_id.is_empty() {
            return true;
        }

        let max_entries = self.max_entries_per_client;
        let outcome = EventOutcomeRecord {
            accepted: true,
            revision_after_effect: 0,
            reject_reason: String::new(),
        };
        if self.clients.contains_key(client_instance_id) {
            self.touch_client(client_instance_id);
            let window = self
                .clients
                .get_mut(client_instance_id)
                .expect("window presence checked above");
            return window.insert_legacy_settled(event_id, outcome, max_entries);
        }

        let mut window = ClientDedupeWindow::new();
        let is_new = window.insert_legacy_settled(event_id, outcome, max_entries);
        if self.make_room_for_new_client().is_err() {
            return false;
        }
        let id = Bytes::copy_from_slice(client_instance_id);
        self.clients.insert(id.clone(), window);
        self.client_order.push_back(id);
        is_new
    }

    /// Convenience helper to record a protobuf [`Event`] through the ID-only API.
    pub fn record_event(&mut self, event: &Event) -> bool {
        self.record(&event.client_instance_id, &event.event_id)
    }

    /// Admits an event into the bounded per-client receive window (§18.2).
    ///
    /// New sequence numbers must be greater than the contiguous processed frontier and no farther
    /// ahead than the configured window. A replay must preserve both `event_id` and
    /// `event_seq`. Invalid admissions do not allocate a new client window.
    pub fn admit_event(&mut self, event: &Event) -> Result<RecordOutcome, EventSequenceError> {
        if event.event_id.is_empty() {
            return Err(EventSequenceError::MissingEventId);
        }

        let max_entries = self.max_entries_per_client;
        if self
            .clients
            .contains_key(event.client_instance_id.as_slice())
        {
            self.touch_client(&event.client_instance_id);
            let window = self
                .clients
                .get_mut(event.client_instance_id.as_slice())
                .expect("window presence checked above");
            return window.admit(&event.event_id, event.event_seq, max_entries);
        }

        // Validated against a throwaway window first, so a refused admission never allocates
        // persistent per-client state and never displaces a retained window (§18.2). The window is
        // seeded from any frontier retained when this client was last evicted, so validation uses
        // the same receive window the client is actually numbering against.
        let mut window = self.window_for_new_client(&event.client_instance_id);
        let restored_frontier = window.last_contiguous_processed_seq;
        let outcome = match window.admit(&event.event_id, event.event_seq, max_entries) {
            Ok(outcome) => outcome,
            Err(error) => {
                // Put the frontier back: a refused admission must not consume the state that a
                // later, valid event still needs (§18.2).
                self.retain_frontier(
                    Bytes::copy_from_slice(&event.client_instance_id),
                    restored_frontier,
                );
                return Err(error);
            }
        };
        if let Err(error) = self.make_room_for_new_client() {
            self.retain_frontier(
                Bytes::copy_from_slice(&event.client_instance_id),
                restored_frontier,
            );
            return Err(error);
        }
        let id = Bytes::copy_from_slice(&event.client_instance_id);
        self.clients.insert(id.clone(), window);
        self.client_order.push_back(id);
        Ok(outcome)
    }

    /// Records the settled outcome and returns the highest contiguous settled event sequence.
    pub fn settle_event(&mut self, event: &Event, outcome: EventOutcomeRecord) -> u64 {
        if event.event_id.is_empty() {
            return self.last_contiguous_processed_seq(&event.client_instance_id);
        }
        self.touch_client(&event.client_instance_id);
        let Some(window) = self.clients.get_mut(event.client_instance_id.as_slice()) else {
            return 0;
        };
        window.settle(&event.event_id, outcome)
    }

    /// Removes an in-flight admission that could not be settled, allowing a later retry to run.
    pub fn abandon_event(&mut self, event: &Event) {
        if event.event_id.is_empty() {
            return;
        }
        let Some(window) = self.clients.get_mut(event.client_instance_id.as_slice()) else {
            return;
        };
        window.abandon(&event.event_id);
    }

    /// Whether this event identity is currently admitted and not yet terminal (§18.2).
    #[must_use]
    pub fn is_in_flight(&self, client_instance_id: &[u8], event_id: &[u8]) -> bool {
        self.clients
            .get(client_instance_id)
            .and_then(|window| window.seen_ids.get(event_id))
            .is_some_and(|record| matches!(record, EventRecord::InFlight { .. }))
    }

    /// Cached terminal outcome for a settled `event_id`, if any.
    #[must_use]
    pub fn settled_outcome(
        &self,
        client_instance_id: &[u8],
        event_id: &[u8],
    ) -> Option<EventOutcomeRecord> {
        self.clients
            .get(client_instance_id)
            .and_then(|window| window.seen_ids.get(event_id))
            .and_then(|record| match record {
                EventRecord::Settled { outcome, .. } => Some(outcome.clone()),
                EventRecord::InFlight { .. } => None,
            })
    }

    /// Admits (if needed) and settles a canceled text-event identity so removing it cannot open
    /// a hole in the contiguous `event_seq` frontier (§18.2, §18.3).
    ///
    /// An already-settled identity is left unchanged and answered from the result cache. An
    /// in-flight identity is settled with `outcome`. A never-seen identity is admitted then
    /// settled so later ordinary events can still advance the frontier through this sequence.
    pub fn settle_canceled_text_event(
        &mut self,
        client_instance_id: &[u8],
        event_id: &[u8],
        event_seq: u64,
        outcome: EventOutcomeRecord,
    ) -> Result<u64, EventSequenceError> {
        if event_id.is_empty() {
            return Err(EventSequenceError::MissingEventId);
        }

        let event = Event {
            client_instance_id: client_instance_id.to_vec(),
            event_seq,
            event_id: event_id.to_vec(),
            ..Default::default()
        };

        match self.admit_event(&event)? {
            RecordOutcome::Duplicate {
                last_processed_event_seq,
                ..
            } => Ok(last_processed_event_seq),
            RecordOutcome::Pending { .. } | RecordOutcome::Fresh { .. } => {
                Ok(self.settle_event(&event, outcome))
            }
        }
    }

    /// Highest contiguous event sequence settled for a client instance (§18.2).
    #[must_use]
    /// Falls back to the frontier retained when the client's window was evicted, so a resume
    /// reports the sequence the client actually reached rather than restarting it at zero (§18.2).
    pub fn last_contiguous_processed_seq(&self, client_instance_id: &[u8]) -> u64 {
        self.clients
            .get(client_instance_id)
            .map(|window| window.last_contiguous_processed_seq)
            .or_else(|| self.evicted_frontiers.get(client_instance_id).copied())
            .unwrap_or(0)
    }

    /// Clears the history for a specific client instance (e.g. when a client terminates).
    ///
    /// Not called on transport detach: §18.2 requires the result cache and frontier to stay valid
    /// for the lifetime of the session incarnation, and a detached client may still resume. Bounded
    /// growth comes from [`DEFAULT_MAX_CLIENT_WINDOWS`] LRU eviction instead.
    pub fn remove_client(&mut self, client_instance_id: &[u8]) {
        self.clients.remove(client_instance_id);
        self.client_order
            .retain(|id| id.as_ref() != client_instance_id);
        // An explicit removal retires the instance outright, so its frontier must go too:
        // retaining it would refuse the sequences a genuinely new instance starts from.
        self.evicted_frontiers.remove(client_instance_id);
        self.frontier_order
            .retain(|id| id.as_ref() != client_instance_id);
    }

    /// Clears all deduplication history across all clients.
    pub fn clear(&mut self) {
        self.clients.clear();
        self.client_order.clear();
        self.evicted_frontiers.clear();
        self.frontier_order.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_event(client: &[u8], event_id: &[u8], event_seq: u64) -> Event {
        Event {
            client_instance_id: client.to_vec(),
            event_seq,
            event_id: event_id.to_vec(),
            ..Default::default()
        }
    }

    fn settle_accepted(dedupe: &mut EventDeduplicator, event: &Event) -> u64 {
        settle_accepted_at_revision(dedupe, event, 0)
    }

    fn settle_accepted_at_revision(
        dedupe: &mut EventDeduplicator,
        event: &Event,
        revision_after_effect: u64,
    ) -> u64 {
        assert!(matches!(
            dedupe.admit_event(event).expect("admit event"),
            RecordOutcome::Fresh { .. }
        ));
        dedupe.settle_event(
            event,
            EventOutcomeRecord {
                accepted: true,
                revision_after_effect,
                reject_reason: String::new(),
            },
        )
    }

    #[test]
    fn test_event_deduplication_basic() {
        let mut dedupe = EventDeduplicator::new(10);
        let client_a = b"client-1";

        assert!(dedupe.record(client_a, b"event-100"));
        assert!(!dedupe.record(client_a, b"event-100"));
        assert!(dedupe.record(client_a, b"event-101"));
        assert!(dedupe.is_duplicate(client_a, b"event-101"));
    }

    /// §26/§27: `client_instance_id` is peer-chosen, so an unbounded client map lets any peer
    /// grow server memory without limit simply by reconnecting under a fresh identity.
    #[test]
    fn test_client_windows_are_bounded_and_evicted_least_recently_used() {
        const MAX_CLIENTS: usize = 4;
        let mut dedupe = EventDeduplicator::with_limits(16, MAX_CLIENTS);

        for client in 0..64u8 {
            let event = wire_event(&[client], b"evt-1", 1);
            settle_accepted(&mut dedupe, &event);
            assert!(
                dedupe.client_count() <= MAX_CLIENTS,
                "retained client instances must stay bounded"
            );
        }

        assert_eq!(dedupe.client_count(), MAX_CLIENTS);
        // The four most recent instances survive; everything older was evicted.
        for client in 60..64u8 {
            assert!(dedupe.is_duplicate(&[client], b"evt-1"));
        }
        assert!(!dedupe.is_duplicate(&[0], b"evt-1"));
    }

    /// §18.2: window exhaustion applies backpressure; it must never evict an `IN_FLIGHT` event.
    #[test]
    fn test_capacity_exhaustion_refuses_rather_than_evicting_in_flight_windows() {
        const MAX_CLIENTS: usize = 2;
        let mut dedupe = EventDeduplicator::with_limits(16, MAX_CLIENTS);

        for client in 0..MAX_CLIENTS as u8 {
            let event = wire_event(&[client], b"evt-1", 1);
            assert!(matches!(
                dedupe.admit_event(&event).expect("admit"),
                RecordOutcome::Fresh { .. }
            ));
        }

        let newcomer = wire_event(b"late", b"evt-1", 1);
        assert_eq!(
            dedupe.admit_event(&newcomer).expect_err("must be refused"),
            EventSequenceError::ClientWindowCapacityExhausted { limit: MAX_CLIENTS }
        );
        assert_eq!(dedupe.client_count(), MAX_CLIENTS);

        // Settling frees a window, and the newcomer is admitted without displacing in-flight work.
        dedupe.settle_event(
            &wire_event(&[0], b"evt-1", 1),
            EventOutcomeRecord {
                accepted: true,
                revision_after_effect: 1,
                reject_reason: String::new(),
            },
        );
        assert!(matches!(
            dedupe.admit_event(&newcomer).expect("admit after settle"),
            RecordOutcome::Fresh { .. }
        ));
    }

    /// §18.2: eviction may drop a client's result cache, but never its place in the sequence
    /// space. Restarting an established client at zero makes its next event permanently invalid.
    #[test]
    fn test_eviction_retains_the_frontier_so_an_established_client_can_keep_sending() {
        const MAX_CLIENTS: usize = 4;
        const MAX_ENTRIES: usize = 16;
        let mut dedupe = EventDeduplicator::with_limits(MAX_ENTRIES, MAX_CLIENTS);

        // An established client works its way well past one window width.
        let veteran = b"veteran";
        for seq in 1..=40u64 {
            let event = wire_event(veteran, format!("evt-{seq}").as_bytes(), seq);
            settle_accepted(&mut dedupe, &event);
        }
        assert_eq!(dedupe.last_contiguous_processed_seq(veteran), 40);

        // Churn fresh identities until the veteran is the least recently used and is evicted.
        for client in 0..6u8 {
            let event = wire_event(&[client], b"evt-1", 1);
            settle_accepted(&mut dedupe, &event);
        }
        assert!(
            !dedupe.is_duplicate(veteran, b"evt-40"),
            "window was evicted"
        );

        // The frontier survives the window, so a resume reports where the client actually is
        // rather than zero.
        assert_eq!(dedupe.last_contiguous_processed_seq(veteran), 40);

        // And its next event is admitted instead of being refused as outside a window that was
        // silently rewound to 1..=16.
        let next = wire_event(veteran, b"evt-41", 41);
        assert!(matches!(
            dedupe
                .admit_event(&next)
                .expect("next event must be admitted"),
            RecordOutcome::Fresh {
                last_processed_event_seq: 40
            }
        ));
    }

    /// The retained frontier is bounded on the same terms as the windows (§26), so a large enough
    /// identity churn ages it out too. Documented rather than fixed: an unbounded frontier map is
    /// exactly the memory exhaustion `max_client_windows` exists to prevent, and a client that has
    /// been idle across two full rounds of the bound is not distinguishable from a new one.
    #[test]
    fn test_a_retained_frontier_ages_out_under_sustained_identity_churn() {
        const MAX_CLIENTS: usize = 4;
        let mut dedupe = EventDeduplicator::with_limits(16, MAX_CLIENTS);

        let veteran = b"veteran";
        for seq in 1..=8u64 {
            let event = wire_event(veteran, format!("evt-{seq}").as_bytes(), seq);
            settle_accepted(&mut dedupe, &event);
        }
        assert_eq!(dedupe.last_contiguous_processed_seq(veteran), 8);

        for client in 0..64u8 {
            let event = wire_event(&[client], b"evt-1", 1);
            settle_accepted(&mut dedupe, &event);
        }

        assert_eq!(
            dedupe.last_contiguous_processed_seq(veteran),
            0,
            "frontier retention is bounded; sustained churn eventually reclaims it"
        );
    }

    /// §18.2: a sequence settled above a gap is not reconstructible from the contiguous frontier,
    /// so its window must be pinned exactly like one holding an in-flight admission.
    #[test]
    fn test_windows_holding_out_of_order_settlements_are_not_evicted() {
        const MAX_CLIENTS: usize = 2;
        let mut dedupe = EventDeduplicator::with_limits(16, MAX_CLIENTS);

        // Two clients, each with a settled sequence 2 while sequence 1 has never arrived: the
        // frontier is still 0, so eviction would lose the only record that 2 already ran.
        for client in 0..MAX_CLIENTS as u8 {
            let event = wire_event(&[client], b"evt-2", 2);
            settle_accepted(&mut dedupe, &event);
            assert_eq!(dedupe.last_contiguous_processed_seq(&[client]), 0);
        }

        let newcomer = wire_event(b"late", b"evt-1", 1);
        assert_eq!(
            dedupe.admit_event(&newcomer).expect_err("must be refused"),
            EventSequenceError::ClientWindowCapacityExhausted { limit: MAX_CLIENTS }
        );

        // The settled out-of-order result is still answerable, so a retry is not re-run.
        assert!(matches!(
            dedupe
                .admit_event(&wire_event(&[0], b"evt-2", 2))
                .expect("replay"),
            RecordOutcome::Duplicate { .. }
        ));
    }

    /// A refused admission must not consume a retained window slot (§18.2).
    #[test]
    fn test_refused_admission_does_not_allocate_a_client_window() {
        let mut dedupe = EventDeduplicator::with_limits(16, 4);
        let invalid = wire_event(b"client-1", b"evt-1", 0);
        assert!(dedupe.admit_event(&invalid).is_err());
        assert_eq!(dedupe.client_count(), 0);
    }

    #[test]
    fn test_event_deduplication_multi_client_isolation() {
        let mut dedupe = EventDeduplicator::new(10);
        let same_event_id = b"event-common";

        assert!(dedupe.record(b"client-a", same_event_id));
        assert!(dedupe.record(b"client-b", same_event_id));
        assert!(!dedupe.record(b"client-a", same_event_id));
        assert!(!dedupe.record(b"client-b", same_event_id));
    }

    #[test]
    fn test_admit_event_stays_pending_until_real_outcome_is_settled() {
        let mut dedupe = EventDeduplicator::new(10);
        let event = wire_event(b"client-1", b"evt-1", 1);

        assert_eq!(
            dedupe.admit_event(&event).expect("fresh admission"),
            RecordOutcome::Fresh {
                last_processed_event_seq: 0
            }
        );
        assert_eq!(
            dedupe.admit_event(&event).expect("pending replay"),
            RecordOutcome::Pending {
                last_processed_event_seq: 0
            }
        );

        assert_eq!(
            dedupe.settle_event(
                &event,
                EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 42,
                    reject_reason: String::new(),
                },
            ),
            1
        );

        match dedupe.admit_event(&event).expect("settled replay") {
            RecordOutcome::Duplicate {
                prior,
                last_processed_event_seq,
            } => {
                assert!(prior.accepted);
                assert_eq!(prior.revision_after_effect, 42);
                assert_eq!(last_processed_event_seq, 1);
            }
            other => panic!("expected Duplicate, got {other:?}"),
        }
    }

    #[test]
    fn test_replay_of_rejected_event_stays_rejected_with_reason() {
        let mut dedupe = EventDeduplicator::new(10);
        let event = wire_event(b"client-1", b"evt-bad", 1);

        assert!(matches!(
            dedupe.admit_event(&event).expect("fresh admission"),
            RecordOutcome::Fresh { .. }
        ));
        dedupe.settle_event(
            &event,
            EventOutcomeRecord {
                accepted: false,
                revision_after_effect: 1,
                reject_reason: "node missing".to_string(),
            },
        );

        match dedupe.admit_event(&event).expect("settled replay") {
            RecordOutcome::Duplicate { prior, .. } => {
                assert!(!prior.accepted);
                assert_eq!(prior.reject_reason, "node missing");
            }
            other => panic!("expected Duplicate, got {other:?}"),
        }
    }

    #[test]
    fn test_contiguous_frontier_waits_for_gap_then_jumps_forward() {
        let mut dedupe = EventDeduplicator::new(10);
        let event_1 = wire_event(b"client-a", b"a1", 1);
        let event_2 = wire_event(b"client-a", b"a2", 2);
        let event_3 = wire_event(b"client-a", b"a3", 3);
        let event_4 = wire_event(b"client-a", b"a4", 4);

        assert_eq!(settle_accepted(&mut dedupe, &event_2), 0);
        assert_eq!(dedupe.last_contiguous_processed_seq(b"client-a"), 0);

        assert_eq!(settle_accepted(&mut dedupe, &event_1), 2);
        assert_eq!(settle_accepted(&mut dedupe, &event_4), 2);
        assert_eq!(settle_accepted(&mut dedupe, &event_3), 4);

        assert_eq!(dedupe.last_contiguous_processed_seq(b"client-b"), 0);
        let client_b_1 = wire_event(b"client-b", b"b1", 1);
        assert_eq!(settle_accepted(&mut dedupe, &client_b_1), 1);
        assert_eq!(dedupe.last_contiguous_processed_seq(b"client-a"), 4);
    }

    #[test]
    fn test_receive_window_is_bounded_and_invalid_event_allocates_nothing() {
        let mut dedupe = EventDeduplicator::new(3);
        let too_far = wire_event(b"client-a", b"a4", 4);

        assert_eq!(
            dedupe.admit_event(&too_far),
            Err(EventSequenceError::OutsideReceiveWindow {
                event_seq: 4,
                first_acceptable_seq: 1,
                last_acceptable_seq: 3,
            })
        );
        assert!(dedupe.clients.is_empty());
    }

    #[test]
    fn test_sequence_and_identity_must_remain_stable() {
        let mut dedupe = EventDeduplicator::new(10);
        let original = wire_event(b"client-a", b"a1", 1);
        assert!(matches!(
            dedupe.admit_event(&original).expect("fresh admission"),
            RecordOutcome::Fresh { .. }
        ));

        let conflicting_id = wire_event(b"client-a", b"other", 1);
        assert_eq!(
            dedupe.admit_event(&conflicting_id),
            Err(EventSequenceError::SequenceAlreadyAssigned { event_seq: 1 })
        );

        let changed_sequence = wire_event(b"client-a", b"a1", 2);
        assert_eq!(
            dedupe.admit_event(&changed_sequence),
            Err(EventSequenceError::ReplaySequenceMismatch {
                expected_event_seq: 1,
                received_event_seq: 2,
            })
        );
    }

    #[test]
    fn test_event_without_id_is_never_deduplicated_or_allocated() {
        let mut dedupe = EventDeduplicator::new(10);
        let event = wire_event(b"client-1", b"", 4);

        assert_eq!(
            dedupe.admit_event(&event),
            Err(EventSequenceError::MissingEventId)
        );
        assert_eq!(
            dedupe.admit_event(&event),
            Err(EventSequenceError::MissingEventId)
        );
        assert_eq!(dedupe.last_contiguous_processed_seq(b"client-1"), 0);
        assert!(dedupe.clients.is_empty());
    }

    #[test]
    fn test_full_receive_window_admits_every_slot_then_refuses_the_next() {
        let mut dedupe = EventDeduplicator::new(4);
        let events: Vec<Event> = (1..=4)
            .map(|seq| wire_event(b"client-a", format!("evt-{seq}").as_bytes(), seq))
            .collect();

        for event in &events {
            assert_eq!(
                dedupe.admit_event(event).expect("admit inside window"),
                RecordOutcome::Fresh {
                    last_processed_event_seq: 0
                }
            );
        }

        let beyond = wire_event(b"client-a", b"evt-5", 5);
        assert_eq!(
            dedupe.admit_event(&beyond),
            Err(EventSequenceError::OutsideReceiveWindow {
                event_seq: 5,
                first_acceptable_seq: 1,
                last_acceptable_seq: 4,
            })
        );

        // Refusing the overflow must not have disturbed a single admitted slot.
        for event in &events {
            assert_eq!(
                dedupe.admit_event(event).expect("in-flight replay"),
                RecordOutcome::Pending {
                    last_processed_event_seq: 0
                }
            );
        }
    }

    #[test]
    fn test_eviction_spares_in_flight_and_out_of_order_settled_entries() {
        let mut dedupe = EventDeduplicator::new(3);
        let first = wire_event(b"client-a", b"a1", 1);
        let second = wire_event(b"client-a", b"a2", 2);
        let third = wire_event(b"client-a", b"a3", 3);
        let fourth = wire_event(b"client-a", b"a4", 4);

        for event in [&first, &second, &third] {
            assert!(matches!(
                dedupe.admit_event(event).expect("admit event"),
                RecordOutcome::Fresh { .. }
            ));
        }
        assert_eq!(
            dedupe.settle_event(
                &first,
                EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 11,
                    reject_reason: String::new(),
                },
            ),
            1
        );
        assert_eq!(
            dedupe.settle_event(
                &third,
                EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 33,
                    reject_reason: String::new(),
                },
            ),
            1
        );

        // Admitting a fourth event needs a slot: only the settled entry behind the frontier is
        // evictable, in FIFO order.
        assert!(matches!(
            dedupe.admit_event(&fourth).expect("admit after eviction"),
            RecordOutcome::Fresh {
                last_processed_event_seq: 1
            }
        ));

        assert_eq!(
            dedupe.admit_event(&second).expect("in-flight survives"),
            RecordOutcome::Pending {
                last_processed_event_seq: 1
            }
        );
        match dedupe
            .admit_event(&third)
            .expect("out-of-order settlement survives")
        {
            RecordOutcome::Duplicate { prior, .. } => {
                assert!(prior.accepted);
                assert_eq!(prior.revision_after_effect, 33);
            }
            other => panic!("expected Duplicate, got {other:?}"),
        }
        // The evicted result is behind the cumulative frontier, so its replay is refused rather
        // than re-executed.
        assert_eq!(
            dedupe.admit_event(&first),
            Err(EventSequenceError::OutsideReceiveWindow {
                event_seq: 1,
                first_acceptable_seq: 2,
                last_acceptable_seq: 4,
            })
        );

        assert_eq!(
            dedupe.settle_event(
                &second,
                EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 22,
                    reject_reason: String::new(),
                },
            ),
            3
        );
        assert_eq!(dedupe.last_contiguous_processed_seq(b"client-a"), 3);
    }

    #[test]
    fn test_zero_and_overflow_sequences_allocate_nothing() {
        let mut dedupe = EventDeduplicator::new(4);

        let zero = wire_event(b"client-a", b"a0", 0);
        assert_eq!(
            dedupe.admit_event(&zero),
            Err(EventSequenceError::OutsideReceiveWindow {
                event_seq: 0,
                first_acceptable_seq: 1,
                last_acceptable_seq: 4,
            })
        );

        let overflow = wire_event(b"client-a", b"a-max", u64::MAX);
        assert_eq!(
            dedupe.admit_event(&overflow),
            Err(EventSequenceError::OutsideReceiveWindow {
                event_seq: u64::MAX,
                first_acceptable_seq: 1,
                last_acceptable_seq: 4,
            })
        );

        assert!(dedupe.clients.is_empty());
        assert_eq!(dedupe.last_contiguous_processed_seq(b"client-a"), 0);

        // A conflicting sequence is refused without disturbing the entry that owns the slot.
        assert_eq!(
            settle_accepted_at_revision(&mut dedupe, &wire_event(b"client-a", b"a1", 1), 7),
            1
        );
        let conflict = wire_event(b"client-a", b"a1-again", 1);
        assert_eq!(
            dedupe.admit_event(&conflict),
            Err(EventSequenceError::SequenceAlreadyAssigned { event_seq: 1 })
        );
        assert!(!dedupe.is_duplicate(b"client-a", b"a1-again"));
        assert_eq!(dedupe.last_contiguous_processed_seq(b"client-a"), 1);
    }

    #[test]
    fn test_sliding_window_eviction() {
        let mut dedupe = EventDeduplicator::new(3);
        let client = b"client-1";

        assert!(dedupe.record(client, b"e1"));
        assert!(dedupe.record(client, b"e2"));
        assert!(dedupe.record(client, b"e3"));
        assert!(dedupe.record(client, b"e4"));

        assert!(!dedupe.is_duplicate(client, b"e1"));
        assert!(dedupe.is_duplicate(client, b"e2"));
        assert!(dedupe.is_duplicate(client, b"e3"));
        assert!(dedupe.is_duplicate(client, b"e4"));
    }

    #[test]
    fn test_canceled_text_event_fills_a_frontier_gap() {
        let mut dedupe = EventDeduplicator::new(16);
        let client = b"client-text";

        let first = wire_event(client, b"ordinary-1", 1);
        assert!(matches!(
            dedupe.admit_event(&first).expect("admit 1"),
            RecordOutcome::Fresh { .. }
        ));

        let canceled = EventOutcomeRecord {
            accepted: false,
            revision_after_effect: 4,
            reject_reason: "canceled on same-session resync".into(),
        };
        assert_eq!(
            dedupe
                .settle_canceled_text_event(client, b"text-2", 2, canceled.clone())
                .expect("cancel seq 2"),
            0,
            "sequence 1 is still in flight, so the frontier must not jump"
        );
        assert!(!dedupe.is_in_flight(client, b"text-2"));
        assert_eq!(
            dedupe.settled_outcome(client, b"text-2"),
            Some(canceled.clone())
        );

        assert_eq!(
            dedupe.settle_event(
                &first,
                EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 1,
                    reject_reason: String::new(),
                }
            ),
            2,
            "settling sequence 1 must advance through the canceled gap"
        );

        // A replay of the canceled identity is answered from the cache, not admitted as fresh.
        let replay = wire_event(client, b"text-2", 2);
        match dedupe.admit_event(&replay).expect("replay") {
            RecordOutcome::Duplicate {
                prior,
                last_processed_event_seq,
            } => {
                assert_eq!(prior, canceled);
                assert_eq!(last_processed_event_seq, 2);
            }
            other => panic!("expected duplicate, got {other:?}"),
        }
    }
}
