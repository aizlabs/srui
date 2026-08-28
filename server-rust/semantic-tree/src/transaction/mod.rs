//! Atomic transactions and monotonically increasing revisions for SemanticStore (§12, §12.1, §12.2, §26).
//!
//! # Architecture & Protocol Invariants
//!
//! - **§12 Persistent Object Graph**: The semantic UI graph persists across transactions; mutations
//!   are incremental rather than full-document resends.
//! - **§12.1 Revisions and Transactions**: Transactions are all-or-nothing atomic units advancing
//!   monotonically from `base_revision` to `new_revision = base_revision + 1`. If any operation
//!   within a transaction fails, the entire transaction is discarded and the store is left in its
//!   exact pre-transaction state without observable side effects.
//! - **§12.2 Commits Are Not Frames**: Commits establish semantic state-consistency boundaries, not
//!   display render cues or repaint pacing. The client/renderer independently schedules drawing.
//! - **§26 Operational Limits**: Transactions pre-check and enforce safety limits, including
//!   `max_transaction_operations`, prior to applying mutations.
//!
//! # Design Decision: Revision Advancement
//!
//! In `SemanticStore::apply_transaction(base_revision, ops)`, the store atomically advances to
//! `new_revision = base_revision + 1` upon full success. For wire transactions or caller-supplied
//! envelopes ([`Transaction`]) where `new_revision` is explicitly supplied, the store validates that
//! `new_revision == base_revision + 1` (rejecting with [`TxnError::InvalidNewRevision`] if mismatched),
//! ensuring strict monotonic increment semantics across all interfaces.

pub mod apply;
pub mod error;
pub mod operation;
pub mod record;
pub mod wire;

pub use error::TxnError;
pub use operation::{Operation, Revision};
pub use record::Transaction;
