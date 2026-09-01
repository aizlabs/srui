//! Wire protocol conversions for operations and transaction envelopes (§16).

use super::error::TxnError;
use super::operation::{Operation, Revision};
use super::record::Transaction;
use crate::ids::{ItemId, ModelId, NodeId, PropertyRef, TypeRef};
use crate::model::ModelItem;
use crate::value::{Property, Value};

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
                let node_type = rec.r#type.map(TypeRef::from).ok_or_else(|| {
                    TxnError::WireError("missing TypeRef in CreateNodeOp".to_string())
                })?;
                let parent_id = if rec.parent_id == 0 {
                    None
                } else {
                    Some(NodeId::new(rec.parent_id))
                };
                let child_index = if rec.child_index == u32::MAX {
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
                let property = set_op.property.map(PropertyRef::from).ok_or_else(|| {
                    TxnError::WireError("missing PropertyRef in SetPropertyOp".to_string())
                })?;
                let value = match set_op.value {
                    Some(v) => {
                        Value::try_from(v).map_err(|e| TxnError::WireError(e.to_string()))?
                    }
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
                let property = clear_op.property.map(PropertyRef::from).ok_or_else(|| {
                    TxnError::WireError("missing PropertyRef in ClearPropertyOp".to_string())
                })?;
                Ok(Self::ClearProperty { id, property })
            }
            Op::MoveNode(move_op) => {
                let id = NodeId::new(move_op.node_id);
                let new_parent_id = if move_op.new_parent_id == 0 {
                    None
                } else {
                    Some(NodeId::new(move_op.new_parent_id))
                };
                let new_child_index = if move_op.new_child_index == u32::MAX {
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
            Op::CreateModel(create_op) => {
                let id = ModelId::new(create_op.model_id);
                let model_type = create_op.model_type.map(TypeRef::from).ok_or_else(|| {
                    TxnError::WireError("missing TypeRef in CreateModelOp".to_string())
                })?;
                Ok(Self::CreateModel {
                    id,
                    model_type,
                    item_count: create_op.item_count,
                })
            }
            Op::ModelInsert(insert_op) => {
                let id = ModelId::new(insert_op.model_id);
                let mut items = Vec::with_capacity(insert_op.items.len());
                for wire_item in insert_op.items {
                    let item = ModelItem::try_from(wire_item)
                        .map_err(|e| TxnError::WireError(e.to_string()))?;
                    items.push(item);
                }
                Ok(Self::ModelInsert {
                    id,
                    index: insert_op.index,
                    items,
                })
            }
            Op::ModelDelete(del_op) => {
                let id = ModelId::new(del_op.model_id);
                let index = if del_op.count > 0 {
                    Some(del_op.index)
                } else {
                    None
                };
                let count = if del_op.count > 0 {
                    Some(del_op.count)
                } else {
                    None
                };
                let item_ids = del_op.item_ids.into_iter().map(ItemId::new).collect();
                Ok(Self::ModelDelete {
                    id,
                    index,
                    count,
                    item_ids,
                })
            }
            Op::ModelUpdate(update_op) => {
                let id = ModelId::new(update_op.model_id);
                let index = if update_op.index == u64::MAX {
                    None
                } else {
                    Some(update_op.index)
                };
                let mut items = Vec::with_capacity(update_op.items.len());
                for wire_item in update_op.items {
                    let item = ModelItem::try_from(wire_item)
                        .map_err(|e| TxnError::WireError(e.to_string()))?;
                    items.push(item);
                }
                Ok(Self::ModelUpdate { id, index, items })
            }
            Op::ModelResetRange(reset_op) => {
                let id = ModelId::new(reset_op.model_id);
                let total_count = if reset_op.total_count > 0 {
                    Some(reset_op.total_count)
                } else {
                    None
                };
                let mut items = Vec::with_capacity(reset_op.items.len());
                for wire_item in reset_op.items {
                    let item = ModelItem::try_from(wire_item)
                        .map_err(|e| TxnError::WireError(e.to_string()))?;
                    items.push(item);
                }
                Ok(Self::ModelResetRange {
                    id,
                    start_index: reset_op.start_index,
                    items,
                    total_count,
                })
            }
            _ => Err(TxnError::WireError(
                "unsupported operation variant for tree mutation".to_string(),
            )),
        }
    }
}

impl From<&Operation> for srui_protocol::Operation {
    fn from(op: &Operation) -> Self {
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
                    .iter()
                    .map(|(p, v)| srui_protocol::Property {
                        property: Some((*p).into()),
                        value: Some(v.into()),
                    })
                    .collect();

                srui_protocol::Operation {
                    op: Some(Op::CreateNode(srui_protocol::CreateNodeOp {
                        node: Some(srui_protocol::NodeRecord {
                            node_id: id.get(),
                            r#type: Some((*node_type).into()),
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
                    property: Some((*property).into()),
                    value: Some(value.into()),
                })),
            },
            Operation::ClearProperty { id, property } => srui_protocol::Operation {
                op: Some(Op::ClearProperty(srui_protocol::ClearPropertyOp {
                    node_id: id.get(),
                    property: Some((*property).into()),
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
                    child_node_ids: new_order.iter().map(|id| id.get()).collect(),
                })),
            },
            Operation::BatchPropertySet { id, properties } => {
                let wire_properties = properties
                    .iter()
                    .map(|(p, v)| srui_protocol::Property {
                        property: Some((*p).into()),
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
            Operation::CreateModel {
                id,
                model_type,
                item_count,
            } => srui_protocol::Operation {
                op: Some(Op::CreateModel(srui_protocol::CreateModelOp {
                    model_id: id.get(),
                    model_type: Some((*model_type).into()),
                    item_count: *item_count,
                })),
            },
            Operation::ModelInsert { id, index, items } => {
                let wire_items = items.iter().map(srui_protocol::ModelItem::from).collect();
                srui_protocol::Operation {
                    op: Some(Op::ModelInsert(srui_protocol::ModelInsertOp {
                        model_id: id.get(),
                        index: *index,
                        items: wire_items,
                    })),
                }
            }
            Operation::ModelDelete {
                id,
                index,
                count,
                item_ids,
            } => srui_protocol::Operation {
                op: Some(Op::ModelDelete(srui_protocol::ModelDeleteOp {
                    model_id: id.get(),
                    index: index.unwrap_or(0),
                    count: count.unwrap_or(0),
                    item_ids: item_ids.iter().map(|i| i.get()).collect(),
                })),
            },
            Operation::ModelUpdate { id, index, items } => {
                let wire_items = items.iter().map(srui_protocol::ModelItem::from).collect();
                srui_protocol::Operation {
                    op: Some(Op::ModelUpdate(srui_protocol::ModelUpdateOp {
                        model_id: id.get(),
                        index: index.unwrap_or(u64::MAX),
                        items: wire_items,
                    })),
                }
            }
            Operation::ModelResetRange {
                id,
                start_index,
                items,
                total_count,
            } => {
                let wire_items = items.iter().map(srui_protocol::ModelItem::from).collect();
                srui_protocol::Operation {
                    op: Some(Op::ModelResetRange(srui_protocol::ModelResetRangeOp {
                        model_id: id.get(),
                        start_index: *start_index,
                        items: wire_items,
                        total_count: total_count.unwrap_or(0),
                    })),
                }
            }
        }
    }
}

impl From<Operation> for srui_protocol::Operation {
    fn from(op: Operation) -> Self {
        (&op).into()
    }
}

impl TryFrom<srui_protocol::Transaction> for Transaction {
    type Error = TxnError;

    fn try_from(wire: srui_protocol::Transaction) -> Result<Self, Self::Error> {
        let mut operations = Vec::with_capacity(wire.operations.len());
        for op in wire.operations {
            operations.push(Operation::try_from(op)?);
        }

        Ok(Self {
            base_revision: Revision::new(wire.base_revision),
            new_revision: Revision::new(wire.new_revision),
            operations,
            priority: wire.priority,
        })
    }
}

impl From<&Transaction> for srui_protocol::Transaction {
    fn from(txn: &Transaction) -> Self {
        let operations = txn
            .operations
            .iter()
            .map(srui_protocol::Operation::from)
            .collect();
        srui_protocol::Transaction {
            base_revision: txn.base_revision.get(),
            new_revision: txn.new_revision.get(),
            operations,
            priority: txn.priority,
        }
    }
}

impl From<Transaction> for srui_protocol::Transaction {
    fn from(txn: Transaction) -> Self {
        (&txn).into()
    }
}
