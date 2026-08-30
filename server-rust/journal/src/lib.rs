//! # SRUI Transaction Journal
//!
//! Maintains a bounded in-memory log of committed transactions (§12, §18, §18.1, §20.2, §21, §32.5).
//! Used to replay state mutations upon client reconnect without resending full snapshots,
//! while bounding memory usage according to [`async-bounded-channel`](rules/async-bounded-channel.md) principles.
//!
//! # Retention policy (§18.1)
//!
//! §18.1 permits several retention policies. The implemented policy is **maximum retained
//! transaction count**: the ring buffer holds at most `max_entries` committed transactions and
//! evicts the oldest on overflow. The bound is configurable per session
//! (`srui_sessiond::SessionConfig::journal_capacity`, `srui-sessiond --journal-capacity`) and
//! defaults to [`DEFAULT_MAX_JOURNAL_ENTRIES`]. The other §18.1 policies — all-client
//! acknowledgement, maximum bytes, revision age, and wall-clock time — are not implemented.
//!
//! A reconnect whose `last_applied_revision` falls outside the retained window cannot be
//! replayed; the server answers `RESYNC_REQUIRED{continuity = SAME_SESSION}` and sends a
//! snapshot instead (§18).
//!
//! Only fully committed transactions are recorded, so replay never emits a partially applied
//! transaction (§12.1).

use srui_protocol::Transaction;
use std::collections::VecDeque;
use thiserror::Error;

/// Default maximum number of historical transactions retained in the journal ring buffer (1024 revisions).
pub const DEFAULT_MAX_JOURNAL_ENTRIES: usize = 1024;

/// Errors returned by [`TransactionJournal`] operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum JournalError {
    /// The transaction's revisions are invalid (e.g. `base_revision + 1 != new_revision`).
    #[error("invalid transaction revision range: base={base}, new={new}")]
    InvalidRevisionRange { base: u64, new: u64 },

    /// The transaction's base revision does not match the journal's current head.
    #[error("non-contiguous revision: expected base {expected}, got {actual}")]
    NonContiguousRevision { expected: u64, actual: u64 },
}

/// Bounded transaction journal for session mutation replay and reconnection catch-up (§20.2, §21).
#[derive(Debug, Clone)]
pub struct TransactionJournal {
    max_entries: usize,
    entries: VecDeque<Transaction>,
    earliest_revision: u64,
    latest_revision: u64,
}

impl Default for TransactionJournal {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_JOURNAL_ENTRIES)
    }
}

impl TransactionJournal {
    /// Creates a new `TransactionJournal` with the specified maximum history capacity.
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            entries: VecDeque::new(),
            earliest_revision: 0,
            latest_revision: 0,
        }
    }

    /// Initializes the journal starting at a specific base revision (e.g. after a snapshot restore).
    pub fn with_initial_revision(max_entries: usize, initial_revision: u64) -> Self {
        Self {
            max_entries: max_entries.max(1),
            entries: VecDeque::new(),
            earliest_revision: initial_revision,
            latest_revision: initial_revision,
        }
    }

    /// Records a newly committed transaction into the journal.
    pub fn record(&mut self, tx: Transaction) -> Result<(), JournalError> {
        if tx.new_revision != tx.base_revision.saturating_add(1) {
            return Err(JournalError::InvalidRevisionRange {
                base: tx.base_revision,
                new: tx.new_revision,
            });
        }

        if !self.entries.is_empty() && tx.base_revision != self.latest_revision {
            return Err(JournalError::NonContiguousRevision {
                expected: self.latest_revision,
                actual: tx.base_revision,
            });
        }

        if self.entries.is_empty() {
            self.earliest_revision = tx.base_revision;
        }

        if self.entries.len() >= self.max_entries {
            if let Some(oldest) = self.entries.pop_front() {
                self.earliest_revision = oldest.new_revision;
            }
        }

        self.latest_revision = tx.new_revision;
        self.entries.push_back(tx);
        Ok(())
    }

    /// Checks whether the journal can replay all transactions starting from `from_revision`.
    #[must_use]
    pub fn can_replay_from(&self, from_revision: u64) -> bool {
        from_revision >= self.earliest_revision && from_revision <= self.latest_revision
    }

    /// Returns an iterator over transactions starting from `from_revision` up to `latest_revision`.
    ///
    /// Returns `None` if `from_revision` falls outside the retained journal window.
    #[must_use]
    pub fn iter_from(&self, from_revision: u64) -> Option<impl Iterator<Item = &Transaction>> {
        if from_revision > self.latest_revision || from_revision < self.earliest_revision {
            return None;
        }

        let skip_count = if from_revision == self.latest_revision {
            self.entries.len()
        } else {
            let count = (from_revision - self.earliest_revision) as usize;
            if count > self.entries.len() {
                return None;
            }
            count
        };

        Some(self.entries.iter().skip(skip_count))
    }

    /// Returns a list of sequential transactions starting from `from_revision` up to `latest_revision`.
    ///
    /// Returns `None` if `from_revision` falls outside the retained journal window (indicating that
    /// the client has fallen too far behind and must receive a full state snapshot resync, §20.2).
    #[must_use]
    pub fn replay_from(&self, from_revision: u64) -> Option<Vec<Transaction>> {
        self.iter_from(from_revision)
            .map(|iter| iter.cloned().collect())
    }

    /// Returns the earliest revision currently retained in the journal.
    #[must_use]
    pub const fn earliest_revision(&self) -> u64 {
        self.earliest_revision
    }

    /// Returns the latest committed revision in the journal.
    #[must_use]
    pub const fn latest_revision(&self) -> u64 {
        self.latest_revision
    }

    /// Returns the number of transactions currently retained in memory.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the journal is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clears all transactions and resets the journal.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.earliest_revision = 0;
        self.latest_revision = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tx(base: u64) -> Transaction {
        Transaction {
            base_revision: base,
            new_revision: base + 1,
            priority: 1,
            operations: vec![],
        }
    }

    #[test]
    fn test_journal_record_and_replay() {
        let mut journal = TransactionJournal::new(10);
        assert_eq!(journal.len(), 0);

        journal.record(make_tx(0)).unwrap();
        journal.record(make_tx(1)).unwrap();
        journal.record(make_tx(2)).unwrap();

        assert_eq!(journal.latest_revision(), 3);
        assert_eq!(journal.earliest_revision(), 0);

        // Replay from rev 0
        let replayed = journal.replay_from(0).unwrap();
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].base_revision, 0);
        assert_eq!(replayed[2].new_revision, 3);

        // Replay from rev 2
        let partial = journal.replay_from(2).unwrap();
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].base_revision, 2);

        // Replay from latest rev returns empty list
        let empty = journal.replay_from(3).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_journal_bounded_eviction_and_gap_detection() {
        let mut journal = TransactionJournal::new(3); // Max 3 entries

        journal.record(make_tx(0)).unwrap(); // Rev 0 -> 1
        journal.record(make_tx(1)).unwrap(); // Rev 1 -> 2
        journal.record(make_tx(2)).unwrap(); // Rev 2 -> 3

        assert_eq!(journal.earliest_revision(), 0);
        assert_eq!(journal.latest_revision(), 3);

        // Insert 4th transaction: evicts rev 0 -> 1
        journal.record(make_tx(3)).unwrap(); // Rev 3 -> 4
        assert_eq!(journal.earliest_revision(), 1);
        assert_eq!(journal.latest_revision(), 4);

        // Replay from rev 0 must return None because it was evicted (gap!)
        assert!(journal.replay_from(0).is_none());
        assert!(!journal.can_replay_from(0));

        // Replay from rev 1 still succeeds
        assert!(journal.can_replay_from(1));
        let replayed = journal.replay_from(1).unwrap();
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].base_revision, 1);
        assert_eq!(replayed[2].new_revision, 4);
    }

    #[test]
    fn test_journal_rejects_non_contiguous_and_invalid_revisions() {
        let mut journal = TransactionJournal::new(10);
        journal.record(make_tx(0)).unwrap();

        // Non-contiguous (expected base 1, got 5)
        let err = journal.record(make_tx(5)).unwrap_err();
        assert!(matches!(
            err,
            JournalError::NonContiguousRevision {
                expected: 1,
                actual: 5
            }
        ));

        // Invalid span (base 1, new 3)
        let bad_tx = Transaction {
            base_revision: 1,
            new_revision: 3,
            priority: 1,
            operations: vec![],
        };
        let err2 = journal.record(bad_tx).unwrap_err();
        assert!(matches!(
            err2,
            JournalError::InvalidRevisionRange { base: 1, new: 3 }
        ));
    }
}
