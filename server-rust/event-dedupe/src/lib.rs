//! # SRUI Event Deduplication
//!
//! Provides sliding-window deduplication for incoming client events (§18.2, §20.2, §21, §32.4).
//! Enforces bounded memory limits per client to prevent memory exhaustion.
//!
//! The window doubles as the bounded **event result cache** of Appendix B:
//! `(client_instance_id, event_id) -> {status, revision_after_effect}`. §18.2 requires that
//! "re-delivery of the same event returns the prior acknowledgement/result", which is only
//! possible if the outcome, not just the id, is retained. The per-client
//! `last_processed_event_seq` high-water mark is the cumulative bound the client reports back in
//! `CLIENT RESUME` (§18).

use std::collections::{HashMap, VecDeque};

use bytes::Bytes;
use srui_protocol::Event;

/// Default maximum number of recent event IDs retained per client instance (4096 events).
pub const DEFAULT_MAX_DEDUPE_ENTRIES: usize = 4096;

/// Settled outcome of one client event, retained for replay answering (§18.2, App. B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventOutcomeRecord {
    /// Whether the event was accepted and dispatched, or refused by validation.
    pub accepted: bool,
    /// Store revision observed after the event's side effects committed (App. B).
    pub revision_after_effect: u64,
}

/// Result of recording an event against a client's dedupe window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The event id had not been seen: the caller must now validate and dispatch it.
    Fresh { last_processed_event_seq: u64 },
    /// The event id was already settled; `prior` is the cached outcome to echo (§18.2).
    Duplicate {
        prior: EventOutcomeRecord,
        last_processed_event_seq: u64,
    },
}

/// Bounded sliding-window event deduplicator per client instance.
#[derive(Debug, Clone)]
pub struct EventDeduplicator {
    max_entries_per_client: usize,
    clients: HashMap<Bytes, ClientDedupeWindow>,
}

#[derive(Debug, Clone)]
struct ClientDedupeWindow {
    seen_ids: HashMap<Bytes, EventOutcomeRecord>,
    order: VecDeque<Bytes>,
    max_processed_seq: u64,
}

impl ClientDedupeWindow {
    fn new() -> Self {
        Self {
            seen_ids: HashMap::new(),
            order: VecDeque::new(),
            max_processed_seq: 0,
        }
    }

    fn record(&mut self, event_id: &[u8], max_entries: usize) -> bool {
        if self.seen_ids.contains_key(event_id) {
            return false; // Duplicate
        }

        if self.order.len() >= max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.seen_ids.remove(&oldest);
            }
        }

        let id = Bytes::copy_from_slice(event_id);
        self.order.push_back(id.clone());
        self.seen_ids.insert(
            id,
            EventOutcomeRecord {
                accepted: true,
                revision_after_effect: 0,
            },
        );
        true
    }

    fn contains(&self, event_id: &[u8]) -> bool {
        self.seen_ids.contains_key(event_id)
    }
}

impl Default for EventDeduplicator {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DEDUPE_ENTRIES)
    }
}

impl EventDeduplicator {
    /// Creates a new `EventDeduplicator` with the specified bounded capacity per client.
    #[must_use]
    pub fn new(max_entries_per_client: usize) -> Self {
        Self {
            max_entries_per_client: max_entries_per_client.max(1),
            clients: HashMap::new(),
        }
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

    /// Records an event. Returns `true` if the event is new (recorded), or `false` if it is a duplicate.
    pub fn record(&mut self, client_instance_id: &[u8], event_id: &[u8]) -> bool {
        if event_id.is_empty() {
            // Events without IDs cannot be deduplicated
            return true;
        }

        let max_entries = self.max_entries_per_client;
        if let Some(window) = self.clients.get_mut(client_instance_id) {
            return window.record(event_id, max_entries);
        }

        let mut window = ClientDedupeWindow::new();
        let is_new = window.record(event_id, max_entries);
        self.clients
            .insert(Bytes::copy_from_slice(client_instance_id), window);
        is_new
    }

    /// Convenience helper to record a protobuf [`Event`].
    pub fn record_event(&mut self, event: &Event) -> bool {
        self.record(&event.client_instance_id, &event.event_id)
    }

    /// Admits an [`Event`] into its client window, reporting whether it is fresh or a replay
    /// of an already-settled event (§18.2).
    ///
    /// A fresh event advances that client instance's `last_processed_event_seq` immediately: the
    /// caller settles it within the same call, as either accepted or rejected, and both are
    /// terminal states as far as the client's retry set is concerned. A duplicate returns the
    /// cached [`EventOutcomeRecord`] so the ack can echo the prior result instead of re-running
    /// the action.
    ///
    /// An event with an empty `event_id` cannot be deduplicated and is always `Fresh`.
    pub fn admit_event(&mut self, event: &Event) -> RecordOutcome {
        let max_entries = self.max_entries_per_client;
        let window = self
            .clients
            .entry(Bytes::copy_from_slice(&event.client_instance_id))
            .or_insert_with(ClientDedupeWindow::new);

        if !event.event_id.is_empty() {
            if let Some(prior) = window.seen_ids.get(event.event_id.as_slice()).copied() {
                return RecordOutcome::Duplicate {
                    prior,
                    last_processed_event_seq: window.max_processed_seq,
                };
            }
            window.record(&event.event_id, max_entries);
        }

        window.max_processed_seq = window.max_processed_seq.max(event.event_seq);
        RecordOutcome::Fresh {
            last_processed_event_seq: window.max_processed_seq,
        }
    }

    /// Records the settled outcome of a previously admitted event (App. B result cache).
    ///
    /// A no-op for an empty `event_id` or an entry already evicted by the sliding window.
    pub fn settle_event(&mut self, event: &Event, outcome: EventOutcomeRecord) {
        if event.event_id.is_empty() {
            return;
        }
        if let Some(window) = self.clients.get_mut(event.client_instance_id.as_slice()) {
            if let Some(slot) = window.seen_ids.get_mut(event.event_id.as_slice()) {
                *slot = outcome;
            }
        }
    }

    /// Highest event sequence settled for a client instance, cumulative across connections (§18).
    #[must_use]
    pub fn max_processed_seq(&self, client_instance_id: &[u8]) -> u64 {
        self.clients
            .get(client_instance_id)
            .map_or(0, |window| window.max_processed_seq)
    }

    /// Clears the history for a specific client instance (e.g. when a client terminates).
    pub fn remove_client(&mut self, client_instance_id: &[u8]) {
        self.clients.remove(client_instance_id);
    }

    /// Clears all deduplication history across all clients.
    pub fn clear(&mut self) {
        self.clients.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_deduplication_basic() {
        let mut dedupe = EventDeduplicator::new(10);
        let client_a = b"client-1";
        let event_1 = b"event-100";
        let event_2 = b"event-101";

        assert!(!dedupe.is_duplicate(client_a, event_1));
        assert!(dedupe.record(client_a, event_1));
        assert!(dedupe.is_duplicate(client_a, event_1));
        assert!(!dedupe.record(client_a, event_1)); // Duplicate!

        assert!(!dedupe.is_duplicate(client_a, event_2));
        assert!(dedupe.record(client_a, event_2));
        assert!(dedupe.is_duplicate(client_a, event_2));
    }

    #[test]
    fn test_event_deduplication_multi_client_isolation() {
        let mut dedupe = EventDeduplicator::new(10);
        let client_a = b"client-a";
        let client_b = b"client-b";
        let same_event_id = b"event-common";

        assert!(dedupe.record(client_a, same_event_id));
        // Client B sending same event ID is not treated as a duplicate of client A
        assert!(dedupe.record(client_b, same_event_id));

        assert!(!dedupe.record(client_a, same_event_id));
        assert!(!dedupe.record(client_b, same_event_id));
    }

    fn wire_event(client: &[u8], event_id: &[u8], event_seq: u64) -> Event {
        Event {
            client_instance_id: client.to_vec(),
            event_seq,
            event_id: event_id.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn test_admit_event_reports_fresh_then_duplicate_with_prior_outcome() {
        let mut dedupe = EventDeduplicator::new(10);
        let event = wire_event(b"client-1", b"evt-1", 7);

        match dedupe.admit_event(&event) {
            RecordOutcome::Fresh {
                last_processed_event_seq,
            } => assert_eq!(last_processed_event_seq, 7),
            other => panic!("expected Fresh, got {:?}", other),
        }

        dedupe.settle_event(
            &event,
            EventOutcomeRecord {
                accepted: true,
                revision_after_effect: 42,
            },
        );

        // §18.2: re-delivery returns the prior result instead of re-running the action.
        match dedupe.admit_event(&event) {
            RecordOutcome::Duplicate {
                prior,
                last_processed_event_seq,
            } => {
                assert!(prior.accepted);
                assert_eq!(prior.revision_after_effect, 42);
                assert_eq!(last_processed_event_seq, 7);
            }
            other => panic!("expected Duplicate, got {:?}", other),
        }
    }

    #[test]
    fn test_replay_of_rejected_event_stays_rejected() {
        let mut dedupe = EventDeduplicator::new(10);
        let event = wire_event(b"client-1", b"evt-bad", 3);

        assert!(matches!(
            dedupe.admit_event(&event),
            RecordOutcome::Fresh { .. }
        ));
        dedupe.settle_event(
            &event,
            EventOutcomeRecord {
                accepted: false,
                revision_after_effect: 1,
            },
        );

        match dedupe.admit_event(&event) {
            RecordOutcome::Duplicate { prior, .. } => assert!(!prior.accepted),
            other => panic!("expected Duplicate, got {:?}", other),
        }
    }

    #[test]
    fn test_max_processed_seq_is_monotonic_and_per_client() {
        let mut dedupe = EventDeduplicator::new(10);

        dedupe.admit_event(&wire_event(b"client-a", b"a1", 5));
        assert_eq!(dedupe.max_processed_seq(b"client-a"), 5);

        // An out-of-order or replayed lower sequence never lowers the high-water mark.
        dedupe.admit_event(&wire_event(b"client-a", b"a2", 2));
        assert_eq!(dedupe.max_processed_seq(b"client-a"), 5);

        dedupe.admit_event(&wire_event(b"client-a", b"a3", 9));
        assert_eq!(dedupe.max_processed_seq(b"client-a"), 9);

        // Scope is the client instance, exactly like the dedupe window itself.
        assert_eq!(dedupe.max_processed_seq(b"client-b"), 0);
        dedupe.admit_event(&wire_event(b"client-b", b"b1", 1));
        assert_eq!(dedupe.max_processed_seq(b"client-b"), 1);
        assert_eq!(dedupe.max_processed_seq(b"client-a"), 9);
        assert_eq!(dedupe.max_processed_seq(b"never-seen"), 0);
    }

    #[test]
    fn test_event_without_id_is_never_deduplicated() {
        let mut dedupe = EventDeduplicator::new(10);
        let event = wire_event(b"client-1", b"", 4);

        assert!(matches!(
            dedupe.admit_event(&event),
            RecordOutcome::Fresh { .. }
        ));
        assert!(matches!(
            dedupe.admit_event(&event),
            RecordOutcome::Fresh { .. }
        ));
        assert_eq!(dedupe.max_processed_seq(b"client-1"), 4);
    }

    #[test]
    fn test_sliding_window_eviction() {
        let mut dedupe = EventDeduplicator::new(3); // Small capacity 3
        let client = b"client-1";

        assert!(dedupe.record(client, b"e1"));
        assert!(dedupe.record(client, b"e2"));
        assert!(dedupe.record(client, b"e3"));

        // All 3 present
        assert!(dedupe.is_duplicate(client, b"e1"));
        assert!(dedupe.is_duplicate(client, b"e2"));
        assert!(dedupe.is_duplicate(client, b"e3"));

        // Insert 4th event -> e1 should be evicted
        assert!(dedupe.record(client, b"e4"));
        assert!(!dedupe.is_duplicate(client, b"e1")); // Evicted!
        assert!(dedupe.is_duplicate(client, b"e2"));
        assert!(dedupe.is_duplicate(client, b"e3"));
        assert!(dedupe.is_duplicate(client, b"e4"));
    }
}
