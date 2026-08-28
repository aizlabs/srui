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

use crate::ids::{NodeId, PropertyRef, TypeRef};
use crate::store::error::StoreError;
use crate::store::SemanticStore;
use crate::value::{Property, Value};
use std::fmt;

/// Monotonically increasing committed semantic state revision (§12.1).
///
/// A revision counter is scoped to a session and never decreases or repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Revision(pub u64);

impl Revision {
    /// Initial session revision (0).
    pub const INITIAL: Self = Self(0);

    /// Constructs a new `Revision`.
    pub const fn new(rev: u64) -> Self {
        Self(rev)
    }

    /// Returns the underlying `u64` numeric revision.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next monotonically increasing revision (`self + 1`).
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Revision({})", self.0)
    }
}

impl From<u64> for Revision {
    fn from(rev: u64) -> Self {
        Self(rev)
    }
}

impl From<Revision> for u64 {
    fn from(rev: Revision) -> Self {
        rev.0
    }
}

/// High-level semantic mutation operation wrapping store primitives (§13).
#[derive(Debug, Clone, PartialEq)]
pub enum Operation {
    /// Creates a new node in the graph (§13 CREATE_NODE, §6.2, §26).
    CreateNode {
        id: NodeId,
        node_type: TypeRef,
        parent_id: Option<NodeId>,
        child_index: Option<usize>,
        properties: Vec<(PropertyRef, Value)>,
    },
    /// Deletes a node and all descendants recursively (§13 DELETE_NODE, §6.2).
    DeleteNode {
        id: NodeId,
    },
    /// Sets or updates a property on a node (§13 SET_PROPERTY, §26).
    SetProperty {
        id: NodeId,
        property: PropertyRef,
        value: Value,
    },
    /// Clears a property from a node (§13 CLEAR_PROPERTY).
    ClearProperty {
        id: NodeId,
        property: PropertyRef,
    },
    /// Moves a node to a new parent or index (§13 MOVE_NODE, §26).
    MoveNode {
        id: NodeId,
        new_parent_id: Option<NodeId>,
        new_child_index: Option<usize>,
    },
    /// Reorders the children of a parent node (§13 REORDER_CHILDREN).
    ReorderChildren {
        parent_id: NodeId,
        new_order: Vec<NodeId>,
    },
    /// Sets multiple properties on a node atomically (§13 BATCH_PROPERTY_SET, §26).
    BatchPropertySet {
        id: NodeId,
        properties: Vec<(PropertyRef, Value)>,
    },
}

impl Operation {
    /// Convenience constructor for [`Operation::CreateNode`].
    pub fn create_node(
        id: NodeId,
        node_type: TypeRef,
        parent_id: Option<NodeId>,
        child_index: Option<usize>,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Self {
        Self::CreateNode {
            id,
            node_type,
            parent_id,
            child_index,
            properties: properties.into_iter().collect(),
        }
    }

    /// Convenience constructor for [`Operation::DeleteNode`].
    pub fn delete_node(id: NodeId) -> Self {
        Self::DeleteNode { id }
    }

    /// Convenience constructor for [`Operation::SetProperty`].
    pub fn set_property(id: NodeId, property: PropertyRef, value: impl Into<Value>) -> Self {
        Self::SetProperty {
            id,
            property,
            value: value.into(),
        }
    }

    /// Convenience constructor for [`Operation::ClearProperty`].
    pub fn clear_property(id: NodeId, property: PropertyRef) -> Self {
        Self::ClearProperty { id, property }
    }

    /// Convenience constructor for [`Operation::MoveNode`].
    pub fn move_node(
        id: NodeId,
        new_parent_id: Option<NodeId>,
        new_child_index: Option<usize>,
    ) -> Self {
        Self::MoveNode {
            id,
            new_parent_id,
            new_child_index,
        }
    }

    /// Convenience constructor for [`Operation::ReorderChildren`].
    pub fn reorder_children(parent_id: NodeId, new_order: impl IntoIterator<Item = NodeId>) -> Self {
        Self::ReorderChildren {
            parent_id,
            new_order: new_order.into_iter().collect(),
        }
    }

    /// Convenience constructor for [`Operation::BatchPropertySet`].
    pub fn batch_property_set(
        id: NodeId,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Self {
        Self::BatchPropertySet {
            id,
            properties: properties.into_iter().collect(),
        }
    }

    /// Applies this operation directly to the given `SemanticStore` (§13).
    pub fn apply(&self, store: &mut SemanticStore) -> Result<(), StoreError> {
        match self {
            Self::CreateNode {
                id,
                node_type,
                parent_id,
                child_index,
                properties,
            } => store.create_node(
                *id,
                *node_type,
                *parent_id,
                *child_index,
                properties.clone(),
            ),
            Self::DeleteNode { id } => store.delete_node(*id).map(|_| ()),
            Self::SetProperty {
                id,
                property,
                value,
            } => store.set_property(*id, *property, value.clone()).map(|_| ()),
            Self::ClearProperty { id, property } => {
                store.clear_property(*id, *property).map(|_| ())
            }
            Self::MoveNode {
                id,
                new_parent_id,
                new_child_index,
            } => store.move_node(*id, *new_parent_id, *new_child_index),
            Self::ReorderChildren {
                parent_id,
                new_order,
            } => store.reorder_children(*parent_id, new_order),
            Self::BatchPropertySet { id, properties } => {
                store.batch_property_set(*id, properties.clone())
            }
        }
    }
}

impl TryFrom<srui_protocol::Operation> for Operation {
    type Error = TxnError;

    fn try_from(op: srui_protocol::Operation) -> Result<Self, Self::Error> {
        use srui_protocol::operation::Op;

        let op_kind = op.op.ok_or_else(|| {
            TxnError::WireError("empty operation payload in wire Operation".to_string())
        })?;

        match op_kind {
            Op::CreateNode(create_op) => {
                let rec = create_op.node.ok_or_else(|| {
                    TxnError::WireError("missing NodeRecord in CreateNodeOp".to_string())
                })?;
                let id = NodeId::new(rec.node_id);
                let node_type = rec
                    .r#type
                    .map(TypeRef::from)
                    .ok_or_else(|| TxnError::WireError("missing TypeRef in CreateNodeOp".to_string()))?;
                let parent_id = if rec.parent_id == 0 {
                    None
                } else {
                    Some(NodeId::new(rec.parent_id))
                };
                let child_index = if parent_id.is_none() || rec.child_index == u32::MAX {
                    None
                } else {
                    Some(rec.child_index as usize)
                };

                let mut properties = Vec::with_capacity(rec.properties.len());
                for wire_prop in rec.properties {
                    let prop = Property::try_from(wire_prop)
                        .map_err(|e| TxnError::WireError(e.to_string()))?;
                    properties.push((prop.property, prop.value));
                }

                Ok(Self::CreateNode {
                    id,
                    node_type,
                    parent_id,
                    child_index,
                    properties,
                })
            }
            Op::DeleteNode(del_op) => Ok(Self::DeleteNode {
                id: NodeId::new(del_op.node_id),
            }),
            Op::SetProperty(set_op) => {
                let id = NodeId::new(set_op.node_id);
                let property = set_op
                    .property
                    .map(PropertyRef::from)
                    .ok_or_else(|| TxnError::WireError("missing PropertyRef in SetPropertyOp".to_string()))?;
                let value = match set_op.value {
                    Some(v) => Value::try_from(v).map_err(|e| TxnError::WireError(e.to_string()))?,
                    None => Value::Null,
                };
                Ok(Self::SetProperty {
                    id,
                    property,
                    value,
                })
            }
            Op::ClearProperty(clear_op) => {
                let id = NodeId::new(clear_op.node_id);
                let property = clear_op
                    .property
                    .map(PropertyRef::from)
                    .ok_or_else(|| TxnError::WireError("missing PropertyRef in ClearPropertyOp".to_string()))?;
                Ok(Self::ClearProperty { id, property })
            }
            Op::MoveNode(move_op) => {
                let id = NodeId::new(move_op.node_id);
                let new_parent_id = if move_op.new_parent_id == 0 {
                    None
                } else {
                    Some(NodeId::new(move_op.new_parent_id))
                };
                let new_child_index = if new_parent_id.is_none() || move_op.new_child_index == u32::MAX {
                    None
                } else {
                    Some(move_op.new_child_index as usize)
                };
                Ok(Self::MoveNode {
                    id,
                    new_parent_id,
                    new_child_index,
                })
            }
            Op::ReorderChildren(reorder_op) => {
                let parent_id = NodeId::new(reorder_op.parent_id);
                let new_order = reorder_op
                    .child_node_ids
                    .into_iter()
                    .map(NodeId::new)
                    .collect();
                Ok(Self::ReorderChildren {
                    parent_id,
                    new_order,
                })
            }
            Op::BatchPropertySet(batch_op) => {
                let id = NodeId::new(batch_op.node_id);
                let mut properties = Vec::with_capacity(batch_op.properties.len());
                for wire_prop in batch_op.properties {
                    let prop = Property::try_from(wire_prop)
                        .map_err(|e| TxnError::WireError(e.to_string()))?;
                    properties.push((prop.property, prop.value));
                }
                Ok(Self::BatchPropertySet { id, properties })
            }
            _ => Err(TxnError::WireError(
                "unsupported operation variant for tree mutation".to_string(),
            )),
        }
    }
}

impl From<Operation> for srui_protocol::Operation {
    fn from(op: Operation) -> Self {
        use srui_protocol::operation::Op;

        match op {
            Operation::CreateNode {
                id,
                node_type,
                parent_id,
                child_index,
                properties,
            } => {
                let wire_properties = properties
                    .into_iter()
                    .map(|(p, v)| srui_protocol::Property {
                        property: Some(p.into()),
                        value: Some(v.into()),
                    })
                    .collect();

                srui_protocol::Operation {
                    op: Some(Op::CreateNode(srui_protocol::CreateNodeOp {
                        node: Some(srui_protocol::NodeRecord {
                            node_id: id.get(),
                            r#type: Some(node_type.into()),
                            parent_id: parent_id.map(|p| p.get()).unwrap_or(0),
                            child_index: child_index.map(|idx| idx as u32).unwrap_or(u32::MAX),
                            properties: wire_properties,
                        }),
                    })),
                }
            }
            Operation::DeleteNode { id } => srui_protocol::Operation {
                op: Some(Op::DeleteNode(srui_protocol::DeleteNodeOp {
                    node_id: id.get(),
                })),
            },
            Operation::SetProperty {
                id,
                property,
                value,
            } => srui_protocol::Operation {
                op: Some(Op::SetProperty(srui_protocol::SetPropertyOp {
                    node_id: id.get(),
                    property: Some(property.into()),
                    value: Some(value.into()),
                })),
            },
            Operation::ClearProperty { id, property } => srui_protocol::Operation {
                op: Some(Op::ClearProperty(srui_protocol::ClearPropertyOp {
                    node_id: id.get(),
                    property: Some(property.into()),
                })),
            },
            Operation::MoveNode {
                id,
                new_parent_id,
                new_child_index,
            } => srui_protocol::Operation {
                op: Some(Op::MoveNode(srui_protocol::MoveNodeOp {
                    node_id: id.get(),
                    new_parent_id: new_parent_id.map(|p| p.get()).unwrap_or(0),
                    new_child_index: new_child_index.map(|idx| idx as u32).unwrap_or(u32::MAX),
                })),
            },
            Operation::ReorderChildren {
                parent_id,
                new_order,
            } => srui_protocol::Operation {
                op: Some(Op::ReorderChildren(srui_protocol::ReorderChildrenOp {
                    parent_id: parent_id.get(),
                    child_node_ids: new_order.into_iter().map(|id| id.get()).collect(),
                })),
            },
            Operation::BatchPropertySet { id, properties } => {
                let wire_properties = properties
                    .into_iter()
                    .map(|(p, v)| srui_protocol::Property {
                        property: Some(p.into()),
                        value: Some(v.into()),
                    })
                    .collect();

                srui_protocol::Operation {
                    op: Some(Op::BatchPropertySet(srui_protocol::BatchPropertySetOp {
                        node_id: id.get(),
                        properties: wire_properties,
                    })),
                }
            }
        }
    }
}

/// An atomic transaction envelope advancing the store from `base_revision` to `new_revision` (§12.1, §16).
#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    /// Committed revision on which this transaction is based.
    pub base_revision: Revision,
    /// Target revision produced upon successful commit (`base_revision + 1`).
    pub new_revision: Revision,
    /// Ordered list of mutation operations to apply atomically.
    pub operations: Vec<Operation>,
    /// Optional scheduling priority class.
    pub priority: u32,
}

impl Transaction {
    /// Constructs a new transaction advancing from `base_revision` to `base_revision + 1`.
    pub fn new(base_revision: impl Into<Revision>, operations: Vec<Operation>) -> Self {
        let base = base_revision.into();
        let new_rev = base.next();
        Self {
            base_revision: base,
            new_revision: new_rev,
            operations,
            priority: 0,
        }
    }

    /// Constructs a new transaction with an explicit priority class.
    pub fn with_priority(
        base_revision: impl Into<Revision>,
        operations: Vec<Operation>,
        priority: u32,
    ) -> Self {
        let base = base_revision.into();
        let new_rev = base.next();
        Self {
            base_revision: base,
            new_revision: new_rev,
            operations,
            priority,
        }
    }

    /// Constructs a transaction with explicitly specified revisions.
    pub fn with_revisions(
        base_revision: impl Into<Revision>,
        new_revision: impl Into<Revision>,
        operations: Vec<Operation>,
        priority: u32,
    ) -> Self {
        Self {
            base_revision: base_revision.into(),
            new_revision: new_revision.into(),
            operations,
            priority,
        }
    }
}

impl TryFrom<srui_protocol::Transaction> for Transaction {
    type Error = TxnError;

    fn try_from(wire: srui_protocol::Transaction) -> Result<Self, Self::Error> {
        let base_revision = Revision::new(wire.base_revision);
        let new_revision = Revision::new(wire.new_revision);
        let mut operations = Vec::with_capacity(wire.operations.len());
        for op in wire.operations {
            operations.push(Operation::try_from(op)?);
        }
        Ok(Self {
            base_revision,
            new_revision,
            operations,
            priority: wire.priority,
        })
    }
}

impl From<Transaction> for srui_protocol::Transaction {
    fn from(txn: Transaction) -> Self {
        let operations = txn.operations.into_iter().map(srui_protocol::Operation::from).collect();
        Self {
            base_revision: txn.base_revision.get(),
            new_revision: txn.new_revision.get(),
            operations,
            priority: txn.priority,
        }
    }
}

/// Errors returned when validating or applying a semantic transaction (§12.1, §26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxnError {
    /// Base revision does not match the store's current committed revision (§12.1).
    StaleBaseRevision {
        expected: Revision,
        actual: Revision,
    },
    /// Caller-supplied `new_revision` is invalid (must be exactly `base_revision + 1` per §12.1).
    InvalidNewRevision {
        expected: Revision,
        actual: Revision,
    },
    /// Transaction exceeds the configured maximum operations limit (§26).
    MaxOperationsExceeded {
        limit: usize,
        actual: usize,
    },
    /// An operation within the transaction failed, causing the entire transaction to be discarded (§12.1).
    OpFailed {
        op_index: usize,
        source: StoreError,
    },
    /// Wire decoding error when parsing protobuf transaction or operations.
    WireError(String),
}

impl fmt::Display for TxnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleBaseRevision { expected, actual } => write!(
                f,
                "base revision mismatch: store is at {}, transaction submitted with base revision {}",
                expected, actual
            ),
            Self::InvalidNewRevision { expected, actual } => write!(
                f,
                "invalid new revision: expected {} (base_revision + 1), but transaction specified {}",
                expected, actual
            ),
            Self::MaxOperationsExceeded { limit, actual } => write!(
                f,
                "transaction operation count {} exceeds configured limit {}",
                actual, limit
            ),
            Self::OpFailed { op_index, source } => {
                write!(f, "operation at index {} failed: {}", op_index, source)
            }
            Self::WireError(msg) => write!(f, "wire transaction error: {}", msg),
        }
    }
}

impl std::error::Error for TxnError {}

impl From<StoreError> for TxnError {
    fn from(err: StoreError) -> Self {
        Self::OpFailed {
            op_index: 0,
            source: err,
        }
    }
}

impl SemanticStore {
    /// Applies a sequence of mutation operations as an atomic transaction advancing from `base_revision` to `base_revision + 1` (§12.1).
    ///
    /// Semantics required by §12.1 and §26:
    /// 1. Rejects if `base_revision` does not match the store's current committed revision (`TxnError::StaleBaseRevision`).
    /// 2. Enforces the §26 `max_transaction_operations` limit as a pre-check before applying any operations (`TxnError::MaxOperationsExceeded`).
    /// 3. Applies all operations speculatively to a private staging copy of the store.
    /// 4. If any operation fails, the entire transaction is discarded with zero visible side-effects or partial changes on the store (`TxnError::OpFailed`).
    /// 5. On full success, atomically commits the staged mutations and advances the store's revision to `new_revision = base_revision + 1`.
    ///
    /// Note (§12.2): Commits establish state-consistency boundaries, not render frames or pacing cues.
    pub fn apply_transaction(
        &mut self,
        base_revision: impl Into<Revision>,
        ops: Vec<Operation>,
    ) -> Result<Revision, TxnError> {
        let base_rev = base_revision.into();
        let current_rev = self.revision();
        if base_rev != current_rev {
            return Err(TxnError::StaleBaseRevision {
                expected: current_rev,
                actual: base_rev,
            });
        }

        let max_ops = self.limits().max_transaction_operations;
        if ops.len() > max_ops {
            return Err(TxnError::MaxOperationsExceeded {
                limit: max_ops,
                actual: ops.len(),
            });
        }

        let mut staged = self.clone_staging();
        for (idx, op) in ops.iter().enumerate() {
            if let Err(source) = op.apply(&mut staged) {
                return Err(TxnError::OpFailed {
                    op_index: idx,
                    source,
                });
            }
        }

        let new_revision = base_rev.next();
        self.commit_staging(staged, new_revision);
        Ok(new_revision)
    }

    /// Applies a structured [`Transaction`] record, validating base revision, new revision (`base_revision + 1`), and operational limits (§12.1).
    pub fn apply_transaction_record(&mut self, txn: &Transaction) -> Result<Revision, TxnError> {
        let current_rev = self.revision();
        if txn.base_revision != current_rev {
            return Err(TxnError::StaleBaseRevision {
                expected: current_rev,
                actual: txn.base_revision,
            });
        }

        let expected_new_rev = txn.base_revision.next();
        if txn.new_revision != expected_new_rev {
            return Err(TxnError::InvalidNewRevision {
                expected: expected_new_rev,
                actual: txn.new_revision,
            });
        }

        let max_ops = self.limits().max_transaction_operations;
        if txn.operations.len() > max_ops {
            return Err(TxnError::MaxOperationsExceeded {
                limit: max_ops,
                actual: txn.operations.len(),
            });
        }

        let mut staged = self.clone_staging();
        for (idx, op) in txn.operations.iter().enumerate() {
            if let Err(source) = op.apply(&mut staged) {
                return Err(TxnError::OpFailed {
                    op_index: idx,
                    source,
                });
            }
        }

        self.commit_staging(staged, txn.new_revision);
        Ok(txn.new_revision)
    }

    /// Decodes and applies a protobuf wire `srui_protocol::Transaction` atomically (§12.1, §16).
    pub fn apply_wire_transaction(
        &mut self,
        wire_txn: &srui_protocol::Transaction,
    ) -> Result<Revision, TxnError> {
        let txn = Transaction::try_from(wire_txn.clone())?;
        self.apply_transaction_record(&txn)
    }
}
