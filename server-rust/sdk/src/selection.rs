//! Server-promoted selection for `List`, `Table` and `Tree` collections (§6.3, §7.4, §7.6, §8,
//! §22.5, §22.7).
//!
//! Selection highlight is local presentation state (§7.6, §22.5) "unless explicitly promoted to
//! semantic state" (§6.3). A server promotes one item by setting the collection's standard
//! `value` property (registry property 13, value type `value`) to that item's [`ItemId`]; no new
//! registry entry is needed. The generic renderer selects that item's row and reveals it once per
//! promotion. The rules the store enforces on every replica live in `srui_semantic_tree::store`;
//! this module adds the authoritative-only rules:
//!
//! - **Unknown item.** A promotion naming an `ItemId` the collection's authoritative model does
//!   not hold is refused with [`StoreError::ItemNotFound`], whether it comes from
//!   [`UiTransaction::promote_selection`] or from a raw `SET_PROPERTY`, `CREATE_NODE` or
//!   `BATCH_PROPERTY_SET` of `value`. The whole transaction is discarded. There is no row-index
//!   fallback.
//! - **selection_mode none.** A collection whose `selection_mode` is `none` or absent refuses the
//!   promotion with [`StoreError::InvalidCollectionSelection`].
//! - **Target outside the replica's cached range.** The promotion carries the item itself in a
//!   one-item `MODEL_RESET_RANGE`, the same authoritative commit that range fulfilment and
//!   server-initiated hydration use (§8, §12.1), so a sparse replica can place the item before it
//!   reveals it. Neighbouring rows then arrive through ordinary viewport range requests.
//! - **Removal.** When a transaction leaves an earlier promotion naming an item its collection's
//!   model no longer holds — through `MODEL_DELETE`, `MODEL_RESET_RANGE` (including cache-cap
//!   eviction), a positional `MODEL_UPDATE`, or a `model_ref` change or clear — the promotion is
//!   cleared in the same commit. A move under sorting (delete and re-insert of the same `ItemId`
//!   in one transaction) keeps it. Deleting the collection node removes its promotion with it.
//!   [`UiTransaction::settle_promoted_selections`] runs on every SDK and sessiond commit path; a
//!   pre-built wire commit (`commit_transaction`) cannot gain operations, so it is refused instead
//!   (see [`dangling_promotions`]).

use std::collections::HashSet;

use crate::session::UiTransaction;
use srui_semantic_tree::{
    is_selectable_collection, ItemId, NodeId, Operation, PropertyRef, SemanticStore,
    StandardSelectionMode, StoreError, Value,
};

/// Returns `true` when `op` can assign a promotion or remove a promoted item from its
/// collection's model, so the transaction's promotions must be re-checked (§6.3, §8).
pub fn affects_promotions(op: &Operation) -> bool {
    let promotion_or_model =
        |p: &PropertyRef| *p == PropertyRef::VALUE || *p == PropertyRef::MODEL_REF;
    match op {
        Operation::SetProperty { property, .. } | Operation::ClearProperty { property, .. } => {
            promotion_or_model(property)
        }
        Operation::BatchPropertySet { properties, .. } => {
            properties.iter().any(|(p, _)| promotion_or_model(p))
        }
        Operation::CreateNode { properties, .. } => {
            properties.iter().any(|(p, _)| *p == PropertyRef::VALUE)
        }
        Operation::ModelDelete { .. }
        | Operation::ModelResetRange { .. }
        | Operation::ModelUpdate { .. } => true,
        _ => false,
    }
}

/// Promotions in `store` whose item the collection's model does not hold: the model is missing,
/// `model_ref` is absent, or the `ItemId` is not in the authoritative model (§6.3, §8).
///
/// Meaningful for an authoritative store. A sparse replica may legitimately not cache the item.
pub fn dangling_promotions(store: &SemanticStore) -> Vec<(NodeId, ItemId)> {
    // lc-debt: O(nodes) scan per commit that touches a model or a promotion; a promotion index
    // maintained by the store would make it O(promotions).
    store
        .iter_nodes()
        .filter_map(|node| {
            let item = node.promoted_selection()?;
            let held = node
                .model_ref()
                .and_then(|model| store.get_model(model))
                .is_some_and(|model| model.contains_item(item));
            (!held).then_some((node.id, item))
        })
        .collect()
}

/// Tracks, within one transaction, whether promotions must be re-checked at commit and which
/// collections this transaction itself assigned a promotion to.
#[derive(Debug, Default)]
pub(crate) struct PromotionTracker {
    /// Some operation could have assigned a promotion or removed a promoted item.
    dirty: bool,
    /// Nodes whose `value` this transaction assigned; a dangling one is refused, not cleared.
    assigned: HashSet<NodeId>,
}

impl PromotionTracker {
    /// Observes one recorded operation.
    pub(crate) fn observe(&mut self, op: &Operation) {
        if !affects_promotions(op) {
            return;
        }
        self.dirty = true;
        let assigns = match op {
            Operation::SetProperty { id, property, .. } if *property == PropertyRef::VALUE => {
                Some(*id)
            }
            Operation::CreateNode { id, properties, .. }
            | Operation::BatchPropertySet { id, properties }
                if properties.iter().any(|(p, _)| *p == PropertyRef::VALUE) =>
            {
                Some(*id)
            }
            _ => None,
        };
        if let Some(id) = assigns {
            self.assigned.insert(id);
        }
    }
}

fn selection_error(node_id: NodeId, reason: &str) -> StoreError {
    StoreError::InvalidCollectionSelection {
        node_id,
        reason: reason.to_string(),
    }
}

impl UiTransaction {
    /// Promotes `item` to the semantic selection of a `List`, `Table` or `Tree` (§6.3, §8).
    ///
    /// Records a one-item `MODEL_RESET_RANGE` carrying the authoritative item, so a replica that
    /// does not cache it can place it, followed by `SET_PROPERTY(value = ItemId)`. The generic
    /// renderer selects the item's row, replacing any local selection, and scrolls it into view
    /// once. Promoting the same `ItemId` again is a new promotion and reveals it again.
    ///
    /// # Errors
    ///
    /// - [`StoreError::NodeNotFound`] when `node` does not exist.
    /// - [`StoreError::InvalidCollectionSelection`] when `node` is not a model-backed `List`,
    ///   `Table` or `Tree`, or its `selection_mode` is `none` or absent.
    /// - [`StoreError::ModelNotFound`] when its `model_ref` names no model.
    /// - [`StoreError::ItemNotFound`] when the authoritative model does not hold `item`. No row
    ///   index is ever substituted.
    pub fn promote_selection(
        &mut self,
        node: impl Into<NodeId>,
        item: impl Into<ItemId>,
    ) -> Result<(), StoreError> {
        let node_id = node.into();
        let item = item.into();
        let node = self
            .staged
            .get_node(node_id)
            .ok_or(StoreError::NodeNotFound(node_id))?;
        if !is_selectable_collection(node.node_type) {
            return Err(selection_error(
                node_id,
                "only a List, Table or Tree holds a promoted selection",
            ));
        }
        let selectable = node
            .get_property(PropertyRef::SELECTION_MODE)
            .and_then(Value::as_enum_token)
            .and_then(|token| StandardSelectionMode::try_from(token).ok())
            .is_some_and(|mode| mode != StandardSelectionMode::None);
        if !selectable {
            return Err(selection_error(
                node_id,
                "selection_mode is none or absent, so no selection can be promoted",
            ));
        }
        let model_id = node.model_ref().ok_or_else(|| {
            selection_error(
                node_id,
                "a promoted selection names a model item, but the node has no model_ref",
            )
        })?;
        let model = self
            .staged
            .get_model(model_id)
            .ok_or(StoreError::ModelNotFound(model_id))?;
        let index = model.index_of(item).ok_or(StoreError::ItemNotFound(item))?;
        let target = model
            .get_item_by_index(index)
            .cloned()
            .ok_or(StoreError::ItemNotFound(item))?;
        let total = model.item_count();

        self.apply_op(&Operation::model_reset_range(
            model_id,
            index,
            [target],
            Some(total),
        ))?;
        self.set(node_id, PropertyRef::VALUE, Value::ItemId(item))?;
        Ok(())
    }

    /// Withdraws a collection's promoted selection, returning the item it named (§6.3).
    ///
    /// The generic renderer clears the native selection when it applies this. A collection with
    /// no promotion records nothing and returns `Ok(None)`.
    pub fn clear_promoted_selection(
        &mut self,
        node: impl Into<NodeId>,
    ) -> Result<Option<ItemId>, StoreError> {
        let node_id = node.into();
        let node = self
            .staged
            .get_node(node_id)
            .ok_or(StoreError::NodeNotFound(node_id))?;
        let Some(previous) = node.promoted_selection() else {
            return Ok(None);
        };
        self.clear(node_id, PropertyRef::VALUE)?;
        Ok(Some(previous))
    }

    /// Returns the promoted selection of a collection in the staging store (§6.3).
    pub fn promoted_selection(&self, node: impl Into<NodeId>) -> Option<ItemId> {
        self.staged.get_node(node.into())?.promoted_selection()
    }

    /// Settles this transaction's promoted selections before it commits (§6.3, §8).
    ///
    /// Runs only when an operation could have assigned a promotion or removed a promoted item.
    /// Every collection whose promotion names an item its authoritative model does not hold is
    /// then either refused — when this transaction assigned that promotion, with
    /// [`StoreError::ItemNotFound`], so the whole transaction is discarded — or cleared in this
    /// same commit, when the promotion predates the transaction and this transaction removed its
    /// item (delete, range reset, positional update, `model_ref` change). A move under sorting
    /// keeps the promotion because the item is present again at commit. Every SDK and sessiond
    /// commit path calls it.
    ///
    /// The tracking state is reset only after success. A refusal, or a clear that fails part-way
    /// (for example at `max_transaction_operations`), leaves it in place, so a caller that calls
    /// this method, ignores the error and lets the transaction commit is refused again by the
    /// commit path's own settlement. After a success, calling it again is a no-op.
    pub fn settle_promoted_selections(&mut self) -> Result<(), StoreError> {
        if !self.promotions.dirty {
            return Ok(());
        }
        let dangling = dangling_promotions(&self.staged);
        if let Some(&(_, item)) = dangling
            .iter()
            .find(|(node, _)| self.promotions.assigned.contains(node))
        {
            return Err(StoreError::ItemNotFound(item));
        }
        for (node_id, _) in dangling {
            // Recording the clear re-marks the tracker dirty; on error it stays dirty.
            self.clear(node_id, PropertyRef::VALUE)?;
        }
        self.promotions = PromotionTracker::default();
        Ok(())
    }
}
