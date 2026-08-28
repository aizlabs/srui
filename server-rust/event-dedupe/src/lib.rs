//! # SRUI Event Deduplication
//!
//! Provides sliding-window deduplication for incoming client events (§18.2, §20.2, §21, §32.4).
//! Enforces bounded memory limits per client to prevent memory exhaustion.

use std::collections::{HashMap, HashSet, VecDeque};

use bytes::Bytes;
use srui_protocol::Event;

/// Default maximum number of recent event IDs retained per client instance (4096 events).
pub const DEFAULT_MAX_DEDUPE_ENTRIES: usize = 4096;

/// Bounded sliding-window event deduplicator per client instance.
#[derive(Debug, Clone)]
pub struct EventDeduplicator {
    max_entries_per_client: usize,
    clients: HashMap<Bytes, ClientDedupeWindow>,
}

#[derive(Debug, Clone)]
struct ClientDedupeWindow {
    seen_ids: HashSet<Bytes>,
    order: VecDeque<Bytes>,
}

impl ClientDedupeWindow {
    fn new() -> Self {
        Self {
            seen_ids: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    fn record(&mut self, event_id: &[u8], max_entries: usize) -> bool {
        if self.seen_ids.contains(event_id) {
            return false; // Duplicate
        }

        if self.order.len() >= max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.seen_ids.remove(&oldest);
            }
        }

        let id = Bytes::copy_from_slice(event_id);
        self.order.push_back(id.clone());
        self.seen_ids.insert(id);
        true
    }

    fn contains(&self, event_id: &[u8]) -> bool {
        self.seen_ids.contains(event_id)
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
