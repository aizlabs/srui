//! Catch-up snapshot export and delivery planning for handshake and resync (§13, §18, §26).

use srui_protocol::{
    framed_payload_len, plan_snapshot_frames, ClientLimits, SnapshotFramePlan, Transaction,
    DEFAULT_MAX_FRAME_SIZE,
};
use srui_semantic_tree::{SemanticStore, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION};

use super::SessionError;

/// Encoded-byte ceiling of the items in one exported `MODEL_RESET_RANGE` (§18, §26).
///
/// The item-count bound alone lets one operation of wide rows outgrow a whole frame, and a
/// snapshot can be split between operations but never inside one. A 1 MiB ceiling keeps every
/// range operation far below `DEFAULT_MAX_FRAME_SIZE`, so split envelopes pack within one operation
/// of the limit.
pub(crate) const SNAPSHOT_MODEL_RANGE_MAX_BYTES: usize = 1 << 20;

/// Plans how `snapshot` reaches a client within every frame limit in force (§18, §26).
///
/// The envelope budget is the smallest of the codec limit this server writes with
/// ([`DEFAULT_MAX_FRAME_SIZE`]), the limit it advertises (`server_max_frame_size`), and the
/// client's advertised `max_frame_size`; a zero advertisement means "no preference". The client's
/// `max_snapshot_parts` (absent/zero = one envelope) bounds the number of envelopes.
///
/// # Errors
///
/// [`SessionError::SnapshotUndeliverable`] when no plan fits, which the caller reports at handshake
/// before any subscription or frame write.
pub(crate) fn plan_snapshot_delivery(
    snapshot: &Transaction,
    server_max_frame_size: u32,
    client_limits: Option<&ClientLimits>,
) -> Result<SnapshotFramePlan, SessionError> {
    let nonzero = |value: u32| (value != 0).then_some(value as usize);
    let max_frame_size = [
        Some(DEFAULT_MAX_FRAME_SIZE),
        nonzero(server_max_frame_size),
        client_limits.and_then(|limits| nonzero(limits.max_frame_size)),
    ]
    .into_iter()
    .flatten()
    .min()
    .expect("the codec limit is always present");
    let max_parts = client_limits.map_or(0, |limits| limits.max_snapshot_parts);
    plan_snapshot_frames(snapshot, max_frame_size, max_parts)
        .map_err(SessionError::SnapshotUndeliverable)
}

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
/// nothing wrong. Splitting the snapshot across envelopes ([`plan_snapshot_delivery`]) lifts only
/// the *byte* bound: the envelopes still form one transaction the replica applies atomically, so
/// its operation count stays bounded by `max_transaction_operations`, and the session fails loudly
/// at handshake instead of silently poisoning every client that attaches to it.
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
                let mut chunk_bytes = 0usize;

                for idx in range.start..range.start + range.length {
                    match model.get_item_by_index(idx) {
                        Some(item) => {
                            let item = srui_protocol::ModelItem::from(item);
                            let item_bytes = framed_payload_len(&item);
                            // §26: a range whose items outgrow the byte ceiling ends here and the
                            // next operation resumes at this index, so no single operation can
                            // outgrow a frame merely because its rows are wide.
                            if !chunk.is_empty()
                                && chunk_bytes + item_bytes > SNAPSHOT_MODEL_RANGE_MAX_BYTES
                            {
                                push_model_reset_range(
                                    &mut ops,
                                    model.id.get(),
                                    chunk_start,
                                    std::mem::take(&mut chunk),
                                );
                                chunk_bytes = 0;
                            }
                            if chunk.is_empty() {
                                chunk_start = idx;
                            }
                            chunk.push(item);
                            chunk_bytes += item_bytes;
                            if chunk.len() == DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION {
                                push_model_reset_range(
                                    &mut ops,
                                    model.id.get(),
                                    chunk_start,
                                    std::mem::take(&mut chunk),
                                );
                                chunk_bytes = 0;
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
                                chunk_bytes = 0;
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

    /// §26: a range of wide rows ends each operation at the byte ceiling, not only at the item
    /// count, so no operation can outgrow a frame merely because its rows are wide.
    #[test]
    fn wide_rows_split_range_operations_at_the_byte_ceiling() {
        use srui_semantic_tree::{ItemId, ModelId, ModelItem, Value};

        let mut store = SemanticStore::new();
        let model = ModelId::new(4);
        store
            .create_model(model, TypeRef::LIST, 3_000)
            .expect("model");
        store
            .model_reset_range(
                model,
                0,
                (0..3_000u64)
                    .map(|i| {
                        ModelItem::with_value(ItemId::new(i + 1), Value::String("w".repeat(1_000)))
                    })
                    .collect(),
                None,
            )
            .expect("rows");

        let snapshot = export_snapshot_transaction(&store).expect("representable");
        let mut next_index = 0u64;
        let mut ranges = 0;
        for op in &snapshot.operations {
            if let Some(srui_protocol::operation::Op::ModelResetRange(range)) = &op.op {
                ranges += 1;
                assert_eq!(range.start_index, next_index, "ranges stay contiguous");
                next_index += range.items.len() as u64;
                let bytes: usize = range.items.iter().map(framed_payload_len).sum();
                assert!(bytes <= SNAPSHOT_MODEL_RANGE_MAX_BYTES);
            }
        }
        assert_eq!(next_index, 3_000);
        assert!(
            ranges >= 3,
            "3 MB of rows must span several operations, got {ranges}"
        );
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
