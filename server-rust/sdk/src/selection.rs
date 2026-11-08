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
//! - **Unknown item.** [`UiTransaction::promote_selection`] refuses an `ItemId` the authoritative
//!   model does not hold with [`StoreError::ItemNotFound`]. There is no row-index fallback.
//! - **selection_mode none.** A collection whose `selection_mode` is `none` or absent refuses the
//!   promotion with [`StoreError::InvalidCollectionSelection`].
//! - **Target outside the replica's cached range.** The promotion carries the item itself in a
//!   one-item `MODEL_RESET_RANGE`, the same authoritative commit that range fulfilment and
//!   server-initiated hydration use (§8, §12.1), so a sparse replica can place the item before it
//!   reveals it. Neighbouring rows then arrive through ordinary viewport range requests.
//! - **Deletion.** A `MODEL_DELETE` that removes the promoted item, and is not followed in the same
//!   transaction by re-inserting that `ItemId` (a move under sorting), clears the promotion in the
//!   same commit: [`UiTransaction::settle_promoted_selections`] runs before every SDK and sessiond
//!   commit.

use crate::session::UiTransaction;
use srui_semantic_tree::{
    is_selectable_collection, ItemId, ModelId, NodeId, Operation, PropertyRef, SemanticStore,
    StandardSelectionMode, StoreError, Value,
};

/// One promotion observed in the staging store: collection node, its model, promoted item.
type Promotion = (NodeId, ModelId, ItemId);

/// Tracks promoted selections that a `MODEL_DELETE` removed within one transaction.
///
/// The promotion index is built lazily on the first `MODEL_DELETE` and invalidated by any node
/// mutation, so a transaction with no model deletions never scans the node graph.
#[derive(Debug, Default)]
pub(crate) struct PromotionTracker {
    /// Promotions in the staging store, or `None` when they must be rescanned.
    index: Option<Vec<Promotion>>,
    /// Promotions whose item a delete in this transaction removed.
    orphaned: Vec<Promotion>,
}

impl PromotionTracker {
    /// Forgets the promotion index after a node mutation.
    pub(crate) fn invalidate(&mut self) {
        self.index = None;
    }

    /// Observes `op` before it is applied to `staged`.
    pub(crate) fn observe(&mut self, staged: &SemanticStore, op: &Operation) {
        match op {
            Operation::ModelDelete {
                id,
                index,
                count,
                item_ids,
            } => {
                // lc-debt: one O(nodes) scan per transaction that deletes model items; an index
                // maintained by the store would make it O(promotions).
                let promotions = self.index.get_or_insert_with(|| scan(staged));
                let Some(model) = staged.get_model(*id) else {
                    return;
                };
                let range = match (index, count) {
                    (start, Some(n)) if *n > 0 => {
                        let start = start.unwrap_or(0);
                        Some(start..start.saturating_add(*n))
                    }
                    _ => None,
                };
                for &(node, model_id, item) in promotions.iter() {
                    if model_id != *id {
                        continue;
                    }
                    let by_id = item_ids.contains(&item);
                    let by_range = range
                        .as_ref()
                        .zip(model.index_of(item))
                        .is_some_and(|(range, at)| range.contains(&at));
                    if by_id || by_range {
                        self.orphaned.push((node, model_id, item));
                    }
                }
            }
            Operation::CreateNode { .. }
            | Operation::DeleteNode { .. }
            | Operation::SetProperty { .. }
            | Operation::ClearProperty { .. }
            | Operation::BatchPropertySet { .. } => self.invalidate(),
            _ => {}
        }
    }
}

fn scan(store: &SemanticStore) -> Vec<Promotion> {
    store
        .iter_nodes()
        .filter_map(|node| Some((node.id, node.model_ref()?, node.promoted_selection()?)))
        .collect()
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

    /// Clears promotions whose item a `MODEL_DELETE` in this transaction removed (§6.3, §8).
    ///
    /// A promotion is kept when its item was re-inserted later in the same transaction, which is
    /// how a move under sorting is expressed. Called by every SDK and sessiond commit path before
    /// the staged store is committed; calling it again is a no-op.
    pub fn settle_promoted_selections(&mut self) -> Result<(), StoreError> {
        let orphaned = std::mem::take(&mut self.promotions.orphaned);
        for (node_id, model_id, item) in orphaned {
            let Some(node) = self.staged.get_node(node_id) else {
                continue;
            };
            if node.promoted_selection() != Some(item) || node.model_ref() != Some(model_id) {
                continue;
            }
            let still_present = self
                .staged
                .get_model(model_id)
                .is_some_and(|model| model.contains_item(item));
            if !still_present {
                self.clear(node_id, PropertyRef::VALUE)?;
            }
        }
        Ok(())
    }
}
