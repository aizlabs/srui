//! Server-hinted keep-in-view for `List`, `Table` and `Tree` items (§4 inv. 13, §6.3, §8,
//! §12.1, §22.5, §22.7).
//!
//! The standard `keep_visible` property names one [`ItemId`] of a model-backed collection that the
//! client keeps in view. It is presentation guidance from the server and never selection state:
//! setting, changing or clearing it never selects, deselects or replaces a local selection, never
//! posts a selection notification and never emits an event. Whenever a commit sets it, or changes
//! the row index at which the item is shown, the generic renderer scrolls the minimum needed to
//! make that row visible; a commit that leaves the row index unchanged never scrolls, so a local
//! pan persists until the item moves. A client without support keeps its viewport where the user
//! put it (the documented fallback, §4 inv. 13).
//!
//! The store enforces the cache-independent rules on every replica (an `ItemId` only, on a
//! `List`, `Table` or `Tree` with a `model_ref`). This module adds the authoritative rules, and
//! [`UiTransaction::settle_promoted_selections`] settles hints exactly like promotions: an
//! assignment naming an item the model does not hold is refused with
//! [`StoreError::ItemNotFound`], and an earlier hint whose item a transaction removed (delete,
//! range reset, positional update, `model_ref` change) is cleared in the same commit.

use crate::session::UiTransaction;
use srui_semantic_tree::{
    is_selectable_collection, ItemId, NodeId, Operation, PropertyRef, StoreError, Value,
};

fn keep_visible_error(node_id: NodeId, reason: &str) -> StoreError {
    StoreError::InvalidKeepVisible {
        node_id,
        reason: reason.to_string(),
    }
}

impl UiTransaction {
    /// Hints that the client keep `item` of a `List`, `Table` or `Tree` in view (§6.3, §8).
    ///
    /// Records a one-item `MODEL_RESET_RANGE` carrying the authoritative item, so a replica that
    /// does not cache it can place it, followed by `SET_PROPERTY(keep_visible = ItemId)`. The
    /// renderer reveals the item's row once for this commit and again whenever a later commit
    /// moves it; it never changes the selection. Setting the same `ItemId` again reveals again.
    ///
    /// # Errors
    ///
    /// - [`StoreError::NodeNotFound`] when `node` does not exist.
    /// - [`StoreError::InvalidKeepVisible`] when `node` is not a `List`, `Table` or `Tree`, or has
    ///   no `model_ref`.
    /// - [`StoreError::ModelNotFound`] when its `model_ref` names no model.
    /// - [`StoreError::ItemNotFound`] when the authoritative model does not hold `item`. No row
    ///   index is ever substituted.
    pub fn set_keep_visible(
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
            return Err(keep_visible_error(
                node_id,
                "keep_visible is admitted only on a model-backed List, Table or Tree",
            ));
        }
        let model_id = node.model_ref().ok_or_else(|| {
            keep_visible_error(
                node_id,
                "keep_visible names a model item, but the node has no model_ref",
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
        self.set(node_id, PropertyRef::KEEP_VISIBLE, Value::ItemId(item))?;
        Ok(())
    }

    /// Clears a collection's `keep_visible` hint, returning the item it named (§6.3).
    ///
    /// The renderer neither scrolls nor changes the selection when it applies this. A collection
    /// without a hint records nothing and returns `Ok(None)`.
    pub fn clear_keep_visible(
        &mut self,
        node: impl Into<NodeId>,
    ) -> Result<Option<ItemId>, StoreError> {
        let node_id = node.into();
        let node = self
            .staged
            .get_node(node_id)
            .ok_or(StoreError::NodeNotFound(node_id))?;
        let Some(previous) = node.keep_visible() else {
            return Ok(None);
        };
        self.clear(node_id, PropertyRef::KEEP_VISIBLE)?;
        Ok(Some(previous))
    }

    /// Returns the `keep_visible` hint of a collection in the staging store (§6.3).
    pub fn keep_visible(&self, node: impl Into<NodeId>) -> Option<ItemId> {
        self.staged.get_node(node.into())?.keep_visible()
    }
}
