//! Catch-up snapshot export for handshake and resync (§13, §18, §26).

use srui_protocol::Transaction;
use srui_semantic_tree::{SemanticStore, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION};

use super::SessionError;

/// Appends one `MODEL_RESET_RANGE` operation carrying `items` starting at `start_index` (§13, §26).
fn push_model_reset_range(
    ops: &mut Vec<srui_protocol::Operation>,
    model_id: u64,
    start_index: u64,
    items: Vec<srui_protocol::ModelItem>,
) {
    ops.push(srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::ModelResetRange(
            srui_protocol::ModelResetRangeOp {
                model_id,
                start_index,
                items,
                total_count: 0,
            },
        )),
    });
}

/// Builds the single `base_revision = 0` transaction that reconstructs `store` (§13, §18).
///
/// # Errors
///
/// Returns [`SessionError::SnapshotUnrepresentable`] when the store needs more operations than
/// §26 `max_transaction_operations` permits in one transaction. This is a *server-side* failure on
/// purpose. Emitting the oversized transaction anyway would put an object on the wire that every
/// conforming replica must reject at decode — both the Swift client
/// (`ProtocolDecoder.validateAndConvertTransaction`) and this repository's own Rust replica
/// (`SemanticStore::apply_staged_owned`) enforce the same bound — and because a reconnect
/// regenerates a byte-identical snapshot, the client would fail, resume, and fail again forever
/// with no diagnosis on either side (§18, §4 inv. 13).
///
/// The store's own `max_node_count` (§26) is an order of magnitude above
/// `max_transaction_operations`, so this bound is reachable by an application that is doing
/// nothing wrong. Restoring service for such a session requires chunked snapshot delivery, which
/// needs an explicit snapshot-framing signal on the wire; until then the session fails loudly at
/// handshake instead of silently poisoning every client that attaches to it.
pub(crate) fn export_snapshot_transaction(
    store: &SemanticStore,
) -> Result<Transaction, SessionError> {
    let max_operations = store.limits().max_transaction_operations;
    let mut ops = Vec::new();

    let mut model_ids: Vec<_> = store.model_ids().collect();
    model_ids.sort_by_key(|id| id.get());
    for model_id in model_ids {
        if let Some(model) = store.get_model(model_id) {
            ops.push(srui_protocol::Operation {
                op: Some(srui_protocol::operation::Op::CreateModel(
                    srui_protocol::CreateModelOp {
                        model_id: model.id.get(),
                        model_type: Some(model.model_type.into()),
                        item_count: model.item_count,
                    },
                )),
            });

            // §26: a model may cache up to `max_cached_items_per_model` (100_000) items, but a
            // single model operation may carry at most `max_items_per_model_operation` (10_000).
            // An unchunked range therefore produces a snapshot that every conforming client must
            // reject — and a rejected resync snapshot leaves the client waiting for a snapshot it
            // will reject again (§18).
            for range in model.cached_ranges() {
                let mut chunk_start = range.start;
                let mut chunk: Vec<srui_protocol::ModelItem> = Vec::new();

                for idx in range.start..range.start + range.length {
                    match model.get_item_by_index(idx) {
                        Some(item) => {
                            if chunk.is_empty() {
                                chunk_start = idx;
                            }
                            chunk.push(srui_protocol::ModelItem::from(item));
                            if chunk.len() == DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION {
                                push_model_reset_range(
                                    &mut ops,
                                    model.id.get(),
                                    chunk_start,
                                    std::mem::take(&mut chunk),
                                );
                            }
                        }
                        // `items` are positional from `start_index`, so a hole must end the run
                        // rather than shift every later item down by one.
                        None => {
                            if !chunk.is_empty() {
                                push_model_reset_range(
                                    &mut ops,
                                    model.id.get(),
                                    chunk_start,
                                    std::mem::take(&mut chunk),
                                );
                            }
                        }
                    }
                }

                if !chunk.is_empty() {
                    push_model_reset_range(&mut ops, model.id.get(), chunk_start, chunk);
                }
            }
        }
    }

    fn visit_node(
        store: &SemanticStore,
        node_id: srui_semantic_tree::NodeId,
        child_index: u32,
        ops: &mut Vec<srui_protocol::Operation>,
    ) {
        if let Some(node) = store.get_node(node_id) {
            let record = srui_protocol::NodeRecord {
                node_id: node.id.get(),
                r#type: Some(node.node_type.into()),
                parent_id: node.parent_id.map(|p| p.get()).unwrap_or(0),
                child_index,
                properties: node
                    .properties
                    .iter()
                    .map(|(p, v)| srui_protocol::Property {
                        property: Some((*p).into()),
                        value: Some(v.clone().into()),
                    })
                    .collect(),
            };
            ops.push(srui_protocol::Operation {
                op: Some(srui_protocol::operation::Op::CreateNode(
                    srui_protocol::CreateNodeOp { node: Some(record) },
                )),
            });

            for (idx, &child_id) in node.ordered_children.iter().enumerate() {
                visit_node(store, child_id, idx as u32, ops);
            }
        }
    }

    for (idx, &root_id) in store.root_ids().iter().enumerate() {
        visit_node(store, root_id, idx as u32, &mut ops);
    }

    if ops.len() > max_operations {
        return Err(SessionError::SnapshotUnrepresentable {
            limit: max_operations,
            actual: ops.len(),
        });
    }

    Ok(Transaction {
        base_revision: 0,
        new_revision: store.revision().get(),
        priority: 0,
        operations: ops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_semantic_tree::{NodeId, TypeRef, DEFAULT_MAX_TRANSACTION_OPERATIONS};

    /// §26: a store larger than one transaction can express must fail here, not on the client.
    #[test]
    fn oversized_store_is_refused_instead_of_emitting_an_unusable_transaction() {
        let mut store = SemanticStore::new();
        let surface = TypeRef::new(0, 1);
        store
            .create_node(NodeId::new(1), surface, None, None, [])
            .expect("root");
        for id in 2..=(DEFAULT_MAX_TRANSACTION_OPERATIONS as u64 + 1) {
            store
                .create_node(NodeId::new(id), surface, Some(NodeId::new(1)), None, [])
                .expect("child");
        }

        match export_snapshot_transaction(&store) {
            Err(SessionError::SnapshotUnrepresentable { limit, actual }) => {
                assert_eq!(limit, DEFAULT_MAX_TRANSACTION_OPERATIONS);
                assert_eq!(actual, DEFAULT_MAX_TRANSACTION_OPERATIONS + 1);
            }
            other => panic!("expected SnapshotUnrepresentable, got {other:?}"),
        }
    }

    /// A store that fits stays expressible, and the emitted snapshot is within §26 bounds.
    #[test]
    fn representable_store_exports_within_the_operation_bound() {
        let mut store = SemanticStore::new();
        let surface = TypeRef::new(0, 1);
        store
            .create_node(NodeId::new(1), surface, None, None, [])
            .expect("root");

        let snapshot = export_snapshot_transaction(&store).expect("representable");
        assert_eq!(snapshot.base_revision, 0);
        assert!(snapshot.operations.len() <= store.limits().max_transaction_operations);
    }
}
