//! Transaction envelope records advancing semantic revisions (§12.1, §16).

use super::operation::{Operation, Revision};

/// An atomic transaction envelope advancing the store from `base_revision` to `new_revision` (§12.1, §16).
#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    /// Committed revision on which this transaction is based.
    pub base_revision: Revision,
    /// Target revision produced upon successful commit (`base_revision + 1`).
    pub new_revision: Revision,
    /// Ordered list of mutation operations to apply atomically.
    pub operations: Vec<Operation>,
    /// Optional scheduling and transport priority class (§16).
    ///
    /// Preserved across wire roundtrips to enable transport-level framing prioritization
    /// and future scheduler queue dispatching.
    pub priority: u32,
}

impl Transaction {
    /// Constructs a standard transaction advancing from `base_revision` to `base_revision + 1`.
    pub fn new(base_revision: Revision, operations: impl IntoIterator<Item = Operation>) -> Self {
        Self {
            base_revision,
            new_revision: base_revision.next(),
            operations: operations.into_iter().collect(),
            priority: 0,
        }
    }

    /// Constructs a transaction with explicit target revision and priority.
    pub fn with_priority(
        base_revision: Revision,
        new_revision: Revision,
        operations: impl IntoIterator<Item = Operation>,
        priority: u32,
    ) -> Self {
        Self::with_revisions(base_revision, new_revision, operations, priority)
    }

    /// Constructs a transaction with explicit base revision, target revision, operations, and priority.
    pub fn with_revisions(
        base_revision: Revision,
        new_revision: Revision,
        operations: impl IntoIterator<Item = Operation>,
        priority: u32,
    ) -> Self {
        Self {
            base_revision,
            new_revision,
            operations: operations.into_iter().collect(),
            priority,
        }
    }
}
