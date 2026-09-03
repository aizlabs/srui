//! Core mutation operations and revision identifiers (§12, §12.1, §13).

use crate::ids::{ItemId, ModelId, NodeId, PropertyRef, TypeRef};
use crate::model::ModelItem;
use crate::store::error::StoreError;
use crate::store::SemanticStore;
use crate::value::Value;
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
    ///
    /// Panics on overflow in debug builds. Use [`Self::checked_next`] for any revision that came
    /// off the wire: a decoded frame may claim `u64::MAX`, which has no successor.
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Returns the next revision, or `None` when this one is exhausted (`u64::MAX`).
    ///
    /// A revision counter never repeats, so `u64::MAX` is the end of a session's sequence rather
    /// than a wrap point: wrapping would hand out a revision the session already used (§12.1).
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
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
    DeleteNode { id: NodeId },
    /// Sets or updates a property on a node (§13 SET_PROPERTY, §26).
    SetProperty {
        id: NodeId,
        property: PropertyRef,
        value: Value,
    },
    /// Clears a property from a node (§13 CLEAR_PROPERTY).
    ClearProperty { id: NodeId, property: PropertyRef },
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
    /// Creates a new collection model (§13 CREATE_MODEL, §8).
    CreateModel {
        id: ModelId,
        model_type: TypeRef,
        item_count: u64,
    },
    /// Inserts items into a collection model at a specified index (§13 MODEL_INSERT, §8).
    ModelInsert {
        id: ModelId,
        index: u64,
        items: Vec<ModelItem>,
    },
    /// Deletes items from a collection model by item identity or index range (§13 MODEL_DELETE, §8).
    ModelDelete {
        id: ModelId,
        index: Option<u64>,
        count: Option<u64>,
        item_ids: Vec<ItemId>,
    },
    /// Updates existing items in a collection model (§13 MODEL_UPDATE, §8).
    ModelUpdate {
        id: ModelId,
        index: Option<u64>,
        items: Vec<ModelItem>,
    },
    /// Resets/replaces a range of cached items in a collection model (§13 MODEL_RESET_RANGE, §8).
    ModelResetRange {
        id: ModelId,
        start_index: u64,
        items: Vec<ModelItem>,
        total_count: Option<u64>,
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
    pub fn reorder_children(
        parent_id: NodeId,
        new_order: impl IntoIterator<Item = NodeId>,
    ) -> Self {
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

    /// Convenience constructor for [`Operation::CreateModel`].
    pub fn create_model(id: ModelId, model_type: TypeRef, item_count: u64) -> Self {
        Self::CreateModel {
            id,
            model_type,
            item_count,
        }
    }

    /// Convenience constructor for [`Operation::ModelInsert`].
    pub fn model_insert(
        id: ModelId,
        index: u64,
        items: impl IntoIterator<Item = ModelItem>,
    ) -> Self {
        Self::ModelInsert {
            id,
            index,
            items: items.into_iter().collect(),
        }
    }

    /// Convenience constructor for [`Operation::ModelDelete`] by item IDs.
    pub fn model_delete_items(id: ModelId, item_ids: impl IntoIterator<Item = ItemId>) -> Self {
        Self::ModelDelete {
            id,
            index: None,
            count: None,
            item_ids: item_ids.into_iter().collect(),
        }
    }

    /// Convenience constructor for [`Operation::ModelDelete`] by index range.
    pub fn model_delete_range(id: ModelId, index: u64, count: u64) -> Self {
        Self::ModelDelete {
            id,
            index: Some(index),
            count: Some(count),
            item_ids: Vec::new(),
        }
    }

    /// Convenience constructor for [`Operation::ModelDelete`].
    pub fn model_delete(
        id: ModelId,
        index: Option<u64>,
        count: Option<u64>,
        item_ids: impl IntoIterator<Item = ItemId>,
    ) -> Self {
        Self::ModelDelete {
            id,
            index,
            count,
            item_ids: item_ids.into_iter().collect(),
        }
    }

    /// Convenience constructor for [`Operation::ModelUpdate`].
    pub fn model_update(
        id: ModelId,
        index: Option<u64>,
        items: impl IntoIterator<Item = ModelItem>,
    ) -> Self {
        Self::ModelUpdate {
            id,
            index,
            items: items.into_iter().collect(),
        }
    }

    /// Convenience constructor for [`Operation::ModelResetRange`].
    pub fn model_reset_range(
        id: ModelId,
        start_index: u64,
        items: impl IntoIterator<Item = ModelItem>,
        total_count: Option<u64>,
    ) -> Self {
        Self::ModelResetRange {
            id,
            start_index,
            items: items.into_iter().collect(),
            total_count,
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
            } => store
                .set_property(*id, *property, value.clone())
                .map(|_| ()),
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
            Self::CreateModel {
                id,
                model_type,
                item_count,
            } => store.create_model(*id, *model_type, *item_count),
            Self::ModelInsert { id, index, items } => {
                store.model_insert(*id, *index, items.clone())
            }
            Self::ModelDelete {
                id,
                index,
                count,
                item_ids,
            } => store.model_delete(*id, *index, *count, item_ids),
            Self::ModelUpdate { id, index, items } => {
                store.model_update(*id, *index, items.clone())
            }
            Self::ModelResetRange {
                id,
                start_index,
                items,
                total_count,
            } => store.model_reset_range(*id, *start_index, items.clone(), *total_count),
        }
    }

    /// Applies this operation to the store by moving heap-allocated fields (no defensive clones).
    pub fn apply_owned(self, store: &mut SemanticStore) -> Result<(), StoreError> {
        match self {
            Self::CreateNode {
                id,
                node_type,
                parent_id,
                child_index,
                properties,
            } => store.create_node(id, node_type, parent_id, child_index, properties),
            Self::DeleteNode { id } => store.delete_node(id).map(|_| ()),
            Self::SetProperty {
                id,
                property,
                value,
            } => store.set_property(id, property, value).map(|_| ()),
            Self::ClearProperty { id, property } => store.clear_property(id, property).map(|_| ()),
            Self::MoveNode {
                id,
                new_parent_id,
                new_child_index,
            } => store.move_node(id, new_parent_id, new_child_index),
            Self::ReorderChildren {
                parent_id,
                new_order,
            } => store.reorder_children(parent_id, &new_order),
            Self::BatchPropertySet { id, properties } => store.batch_property_set(id, properties),
            Self::CreateModel {
                id,
                model_type,
                item_count,
            } => store.create_model(id, model_type, item_count),
            Self::ModelInsert { id, index, items } => store.model_insert(id, index, items),
            Self::ModelDelete {
                id,
                index,
                count,
                item_ids,
            } => store.model_delete(id, index, count, &item_ids),
            Self::ModelUpdate { id, index, items } => store.model_update(id, index, items),
            Self::ModelResetRange {
                id,
                start_index,
                items,
                total_count,
            } => store.model_reset_range(id, start_index, items, total_count),
        }
    }
}
