//! # SRUI Event Deduplication
//!
//! Provides sliding-window deduplication for incoming client events (§18.2, §20.2, §21, §32.4).
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

/// Bounded sliding-window event deduplicator per client instance.
#[derive(Debug, Clone)]
pub struct EventDeduplicator {
    max_entries_per_client: usize,
    clients: HashMap<Bytes, ClientDedupeWindow>,
}

#[derive(Debug, Clone)]
enum EventRecord {
    Pending {
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
            Self::Pending { event_seq } | Self::Settled { event_seq, .. } => *event_seq,
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
                EventRecord::Pending { .. } => RecordOutcome::Pending {
                    last_processed_event_seq: self.last_contiguous_processed_seq,
                },
                EventRecord::Settled { outcome, .. } => RecordOutcome::Duplicate {
                    prior: outcome.clone(),
                    last_processed_event_seq: self.last_contiguous_processed_seq,
                },
            });
        }

        let first_acceptable_seq = self.last_contiguous_processed_seq.saturating_add(1);
        let window_width = u64::try_from(max_entries).unwrap_or(u64::MAX);
        let last_acceptable_seq = self
            .last_contiguous_processed_seq
            .saturating_add(window_width);
        if event_seq <= self.last_contiguous_processed_seq || event_seq > last_acceptable_seq {
            return Err(EventSequenceError::OutsideReceiveWindow {
                event_seq,
                first_acceptable_seq,
                last_acceptable_seq,
            });
        }
        if self.ids_by_seq.contains_key(&event_seq) {
            return Err(EventSequenceError::SequenceAlreadyAssigned { event_seq });
        }

        while self.seen_ids.len() >= max_entries {
            if !self.evict_oldest_settled() {
                return Err(EventSequenceError::ReceiveWindowFull);
            }
        }

        let id = Bytes::copy_from_slice(event_id);
        self.order.push_back(id.clone());
        self.ids_by_seq.insert(event_seq, id.clone());
        self.seen_ids.insert(id, EventRecord::Pending { event_seq });

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
                EventRecord::Pending { event_seq } => {
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
            EventRecord::Pending { event_seq } => Some(*event_seq),
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
        if let Some(window) = self.clients.get_mut(client_instance_id) {
            return window.insert_legacy_settled(event_id, outcome, max_entries);
        }

        let mut window = ClientDedupeWindow::new();
        let is_new = window.insert_legacy_settled(event_id, outcome, max_entries);
        self.clients
            .insert(Bytes::copy_from_slice(client_instance_id), window);
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
        if let Some(window) = self.clients.get_mut(event.client_instance_id.as_slice()) {
            return window.admit(&event.event_id, event.event_seq, max_entries);
        }

        let mut window = ClientDedupeWindow::new();
        let outcome = window.admit(&event.event_id, event.event_seq, max_entries)?;
        self.clients
            .insert(Bytes::copy_from_slice(&event.client_instance_id), window);
        Ok(outcome)
    }

    /// Records the settled outcome and returns the highest contiguous settled event sequence.
    pub fn settle_event(&mut self, event: &Event, outcome: EventOutcomeRecord) -> u64 {
        if event.event_id.is_empty() {
            return self.last_contiguous_processed_seq(&event.client_instance_id);
        }
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

    /// Highest contiguous event sequence settled for a client instance (§18.2).
    #[must_use]
    pub fn last_contiguous_processed_seq(&self, client_instance_id: &[u8]) -> u64 {
        self.clients
            .get(client_instance_id)
            .map_or(0, |window| window.last_contiguous_processed_seq)
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

    fn wire_event(client: &[u8], event_id: &[u8], event_seq: u64) -> Event {
        Event {
            client_instance_id: client.to_vec(),
            event_seq,
            event_id: event_id.to_vec(),
            ..Default::default()
        }
    }

    fn settle_accepted(dedupe: &mut EventDeduplicator, event: &Event) -> u64 {
        assert!(matches!(
            dedupe.admit_event(event).expect("admit event"),
            RecordOutcome::Fresh { .. }
        ));
        dedupe.settle_event(
            event,
            EventOutcomeRecord {
                accepted: true,
                revision_after_effect: 0,
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
}
