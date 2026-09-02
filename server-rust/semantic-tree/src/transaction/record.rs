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

    /// Returns `true` if all operations in this transaction are scalar `SetProperty` operations (§7.6, §20.2).
    pub fn is_coalesceable(&self) -> bool {
        !self.operations.is_empty()
            && self.operations.iter().all(|op| match op {
                Operation::SetProperty { value, .. } => value.is_scalar(),
                _ => false,
            })
    }

    /// Attempts to absorb an adjacent transaction into `self` if both are contiguous,
    /// have matching priority, consist entirely of scalar `SetProperty` mutations,
    /// and the merged result stays within `max_ops` and `max_frame_size` (§20.2, §26).
    ///
    /// Retains the latest value per `(NodeId, PropertyRef)`.
    pub fn try_absorb(
        &mut self,
        incoming: &Transaction,
        max_ops: usize,
        max_frame_size: usize,
    ) -> bool {
        if self.priority != incoming.priority {
            return false;
        }
        if self.new_revision != incoming.base_revision {
            return false;
        }
        if !self.is_coalesceable() || !incoming.is_coalesceable() {
            return false;
        }

        use crate::ids::{NodeId, PropertyRef};
        use prost::Message;
        use std::collections::HashMap;

        let mut key_map: HashMap<(NodeId, PropertyRef), usize> =
            HashMap::with_capacity(self.operations.len());
        for (idx, op) in self.operations.iter().enumerate() {
            if let Operation::SetProperty { id, property, .. } = op {
                key_map.insert((*id, *property), idx);
            }
        }

        let mut new_keys_count = 0;
        for op in &incoming.operations {
            if let Operation::SetProperty { id, property, .. } = op {
                key_map.entry((*id, *property)).or_insert_with(|| {
                    new_keys_count += 1;
                    usize::MAX
                });
            }
        }

        if self.operations.len() + new_keys_count > max_ops {
            return false;
        }

        let mut merged_ops = self.operations.clone();
        key_map.clear();
        for (idx, op) in merged_ops.iter().enumerate() {
            if let Operation::SetProperty { id, property, .. } = op {
                key_map.insert((*id, *property), idx);
            }
        }

        for op in &incoming.operations {
            if let Operation::SetProperty {
                id,
                property,
                value,
            } = op
            {
                let key = (*id, *property);
                if let Some(&idx) = key_map.get(&key) {
                    merged_ops[idx] = Operation::SetProperty {
                        id: *id,
                        property: *property,
                        value: value.clone(),
                    };
                } else {
                    let idx = merged_ops.len();
                    merged_ops.push(Operation::SetProperty {
                        id: *id,
                        property: *property,
                        value: value.clone(),
                    });
                    key_map.insert(key, idx);
                }
            }
        }

        let test_txn = Transaction {
            base_revision: self.base_revision,
            new_revision: incoming.new_revision,
            operations: merged_ops,
            priority: self.priority,
        };
        let wire: srui_protocol::Transaction = (&test_txn).into();
        if wire.encoded_len() > max_frame_size {
            return false;
        }

        self.operations = test_txn.operations;
        self.new_revision = incoming.new_revision;
        true
    }
}
