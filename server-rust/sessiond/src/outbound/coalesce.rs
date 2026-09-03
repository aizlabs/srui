//! Scalar coalescing for outbound delivery queues (§12.1, §20.2, §20.4).
//!
//! Coalescing is a delivery policy, not a property of the authoritative transaction model: a slow
//! client may be sent one envelope standing in for a run of committed scalar updates, and the
//! result is a [`CoalescedScalarDelta`] — non-authoritative by construction, never journalled
//! (§12.1).
//!
//! The rules enforced here:
//!
//! - only contiguous transactions merge (`tail.new_revision == incoming.base_revision`), so the
//!   delta stays equivalent to applying the spanned transactions in order;
//! - both sides must be scalar-only; any structural operation is a barrier and keeps its own
//!   revision boundary;
//! - priority classes never merge across (§16);
//! - the merged result stays within the negotiated `max_transaction_operations` and
//!   `max_frame_size` bounds (§26), measured on the enclosing `SruiMessage` that will carry it.

use std::collections::HashMap;

use srui_semantic_tree::{
    CoalescedScalarDelta, NodeId, Operation, PropertyRef, Transaction as DomainTxn,
};

/// Merges `incoming` into `tail` in place, keeping the latest value per `(node, property)`.
///
/// Returns `false` and leaves `tail` untouched when the two may not be coalesced, in which case the
/// caller must queue `incoming` as its own item.
pub(crate) fn try_absorb(
    tail: &mut DomainTxn,
    incoming: &DomainTxn,
    max_ops: usize,
    max_frame_size: usize,
) -> bool {
    if tail.priority != incoming.priority {
        return false;
    }
    if tail.new_revision != incoming.base_revision {
        return false;
    }
    if !tail.is_coalesceable() || !incoming.is_coalesceable() {
        return false;
    }

    let mut key_map: HashMap<(NodeId, PropertyRef), usize> =
        HashMap::with_capacity(tail.operations.len() + incoming.operations.len());
    for (idx, op) in tail.operations.iter().enumerate() {
        if let Operation::SetProperty { id, property, .. } = op {
            key_map.insert((*id, *property), idx);
        }
    }

    let new_keys_count = incoming
        .operations
        .iter()
        .filter(|op| {
            matches!(
                op,
                Operation::SetProperty { id, property, .. }
                    if !key_map.contains_key(&(*id, *property))
            )
        })
        .count();

    if tail.operations.len() + new_keys_count > max_ops {
        return false;
    }

    let mut merged_ops = tail.operations.clone();
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

    let merged = DomainTxn {
        base_revision: tail.base_revision,
        new_revision: incoming.new_revision,
        operations: merged_ops,
        priority: tail.priority,
    };

    // §26 is measured on the frame that will actually carry this transaction, not on the bare
    // transaction: the envelope's own tag and length header count against the limit.
    let wire: srui_protocol::Transaction = (&merged).into();
    let envelope = srui_protocol::SruiMessage {
        msg: Some(srui_protocol::srui_message::Msg::Transaction(wire)),
    };
    if srui_protocol::framed_payload_len(&envelope) > max_frame_size {
        return false;
    }

    // The merged result is a delivery delta by construction: contiguous, forward, scalar-only.
    debug_assert!(
        CoalescedScalarDelta::validate(&merged).is_ok(),
        "coalescing produced a transaction that is not a valid delivery delta"
    );

    tail.operations = merged.operations;
    tail.new_revision = merged.new_revision;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_semantic_tree::{Revision, Value};

    fn scalar(
        base: u64,
        new_rev: u64,
        node_id: u64,
        property: PropertyRef,
        value: Value,
    ) -> DomainTxn {
        DomainTxn::with_revisions(
            Revision::new(base),
            Revision::new(new_rev),
            vec![Operation::SetProperty {
                id: NodeId::new(node_id),
                property,
                value,
            }],
            0,
        )
    }

    #[test]
    fn test_try_absorb_semantics_and_limits() {
        let node_id = 10;
        let mut t1 = scalar(1, 2, node_id, PropertyRef::LABEL, Value::from("first"));
        let t2 = scalar(2, 3, node_id, PropertyRef::LABEL, Value::from("second"));

        // Contiguous scalar absorption succeeds and updates target revision
        assert!(try_absorb(&mut t1, &t2, 100, 1024 * 1024));
        assert_eq!(t1.base_revision, Revision::new(1));
        assert_eq!(t1.new_revision, Revision::new(3));
        assert_eq!(t1.operations.len(), 1);
        match &t1.operations[0] {
            Operation::SetProperty { value, .. } => assert_eq!(value, &Value::from("second")),
            other => panic!("expected SetProperty, got {other:?}"),
        }

        // Structural transaction cannot be absorbed
        let t_structural = DomainTxn::with_revisions(
            Revision::new(3),
            Revision::new(4),
            vec![Operation::DeleteNode {
                id: NodeId::new(node_id),
            }],
            0,
        );
        assert!(!try_absorb(&mut t1, &t_structural, 100, 1024 * 1024));

        // Exceeding max_ops is rejected
        let t3 = scalar(3, 4, node_id, PropertyRef::VALUE, Value::from(42.0));
        assert!(!try_absorb(&mut t1, &t3, 1, 1024 * 1024));

        // Exceeding max_frame_size is rejected
        assert!(!try_absorb(&mut t1, &t3, 100, 5));
    }

    /// A delta must stay equivalent to applying the transactions it spans, so a gap in the revision
    /// sequence must not be merged away (§12.1).
    #[test]
    fn test_non_contiguous_transactions_do_not_absorb() {
        let mut tail = scalar(1, 2, 10, PropertyRef::LABEL, Value::from("first"));
        let disjoint = scalar(7, 8, 10, PropertyRef::LABEL, Value::from("later"));

        assert!(!try_absorb(&mut tail, &disjoint, 100, 1024 * 1024));
        assert_eq!(tail.new_revision, Revision::new(2));
    }

    /// Priority is a scheduling class (§16): transactions in different classes may not be collapsed
    /// into one envelope.
    #[test]
    fn test_priority_classes_do_not_absorb() {
        let mut tail = scalar(1, 2, 10, PropertyRef::LABEL, Value::from("first"));
        let mut other = scalar(2, 3, 10, PropertyRef::LABEL, Value::from("second"));
        other.priority = 7;

        assert!(!try_absorb(&mut tail, &other, 100, 1024 * 1024));
        assert_eq!(tail.new_revision, Revision::new(2));
    }
}
