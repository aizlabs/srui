//! Mutable authoritative in-memory semantic graph store (§6.2, §6.3, §13, §26).

pub mod error;
pub mod limits;
pub mod node;

pub use error::StoreError;
pub use limits::{StoreLimits, DEFAULT_MAX_TRANSACTION_OPERATIONS};
pub use node::Node;

use crate::ids::{ItemId, ModelId, NodeId, PropertyRef, TypeRef};
use crate::model::{Model, ModelItem};
use crate::transaction::Revision;
use crate::value::{Property, Value};
use std::collections::{HashMap, HashSet};

/// Default maximum allowed tree depth (§26).
pub const DEFAULT_MAX_TREE_DEPTH: usize = 64;
/// Default maximum allowed total node count (§26).
pub const DEFAULT_MAX_NODE_COUNT: usize = 100_000;
/// Default maximum allowed property string length in bytes (§26).
pub const DEFAULT_MAX_STRING_LENGTH: usize = 1024 * 1024; // 1 MiB

/// Authoritative in-memory semantic node graph holding the session's active UI tree (§6.2, §6.3, §12).
///
/// Note: per §6.3, the server `SemanticStore` is the single authoritative source of truth for the
/// session graph and intentionally does not implement `Clone` to prevent accidental divergent tree copies.
#[derive(Debug)]
pub struct SemanticStore {
    /// Active nodes mapped by `NodeId`.
    nodes: HashMap<NodeId, Node>,
    /// Top-level root node IDs (`parent_id == None`), in insertion order.
    roots: Vec<NodeId>,
    /// Set of all `NodeId`s that have ever been created in this session (§6.2 invariant).
    used_ids: HashSet<NodeId>,
    /// Active collection models mapped by `ModelId` (§8, §13).
    models: HashMap<ModelId, Model>,
    /// Set of all `ModelId`s that have ever been created in this session (§6.2, §8 invariant).
    used_model_ids: HashSet<ModelId>,
    /// Mandatory runtime limits enforced by the store (§26).
    limits: StoreLimits,
    /// Authoritative committed revision counter (§12.1).
    revision: Revision,
}

impl Default for SemanticStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticStore {
    /// Constructs a new `SemanticStore` with default limits and baseline revision 0 (§12.1, §26).
    pub fn new() -> Self {
        Self::with_limits(StoreLimits::default())
    }

    /// Constructs a new `SemanticStore` with configured limits and baseline revision 0 (§12.1, §26).
    pub fn with_limits(limits: StoreLimits) -> Self {
        Self {
            nodes: HashMap::new(),
            roots: Vec::new(),
            used_ids: HashSet::new(),
            models: HashMap::new(),
            used_model_ids: HashSet::new(),
            limits,
            revision: Revision::INITIAL,
        }
    }

    /// Constructs a new `SemanticStore` with configured limits and initial committed revision (§12.1, §18).
    pub fn with_limits_and_revision(limits: StoreLimits, revision: Revision) -> Self {
        Self {
            nodes: HashMap::new(),
            roots: Vec::new(),
            used_ids: HashSet::new(),
            models: HashMap::new(),
            used_model_ids: HashSet::new(),
            limits,
            revision,
        }
    }

    /// Returns the store's current committed revision (§12.1).
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Returns the store's current committed revision (§12.1).
    pub fn committed_revision(&self) -> Revision {
        self.revision
    }

    /// Creates a private staging clone of the store's node graph for atomic transaction application (§12.1).
    pub(crate) fn clone_staging(&self) -> Self {
        Self {
            nodes: self.nodes.clone(),
            roots: self.roots.clone(),
            used_ids: self.used_ids.clone(),
            models: self.models.clone(),
            used_model_ids: self.used_model_ids.clone(),
            limits: self.limits.clone(),
            revision: self.revision,
        }
    }

    /// Atomically commits a successful staging store and advances the committed revision (§12.1).
    pub(crate) fn commit_staging(&mut self, staged: Self, new_revision: Revision) {
        self.nodes = staged.nodes;
        self.roots = staged.roots;
        self.used_ids = staged.used_ids;
        self.models = staged.models;
        self.used_model_ids = staged.used_model_ids;
        self.revision = new_revision;
    }

    /// Returns a reference to the store's configured limits.
    pub fn limits(&self) -> &StoreLimits {
        &self.limits
    }

    /// Returns the number of active nodes currently in the store.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if the store contains no active nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns `true` if an active node exists with the given ID.
    pub fn contains_node(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Returns `true` if the given `NodeId` was ever used in this session (even if deleted, §6.2).
    pub fn is_id_used(&self, id: NodeId) -> bool {
        self.used_ids.contains(&id)
    }

    /// Returns an immutable reference to the node with the given ID.
    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Returns a mutable reference to the node with the given ID.
    pub fn get_node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// Returns a slice of the top-level root node IDs.
    pub fn root_ids(&self) -> &[NodeId] {
        &self.roots
    }

    /// Returns a slice of the ordered child IDs for the given parent node.
    pub fn children_of(&self, parent_id: NodeId) -> Option<&[NodeId]> {
        self.nodes.get(&parent_id).map(|n| n.ordered_children.as_slice())
    }

    /// Returns the parent ID of the given node, if it exists and has a parent.
    pub fn parent_of(&self, id: NodeId) -> Option<Option<NodeId>> {
        self.nodes.get(&id).map(|n| n.parent_id)
    }

    /// Calculates the depth of a node in the hierarchy (root is depth 1).
    pub fn node_depth(&self, id: NodeId) -> Option<usize> {
        self.calculate_node_depth(id)
    }

    /// Calculates the depth of a node in the hierarchy (root is depth 1).
    pub fn calculate_node_depth(&self, id: NodeId) -> Option<usize> {
        let mut current_id = id;
        let mut depth = 0;
        loop {
            let node = self.nodes.get(&current_id)?;
            depth += 1;
            match node.parent_id {
                Some(parent) => current_id = parent,
                None => break,
            }
        }
        Some(depth)
    }

    /// Calculates the maximum depth of any node within the subtree rooted at `id` (relative to `id`, root of subtree is 1).
    pub fn subtree_depth(&self, id: NodeId) -> usize {
        self.calculate_subtree_max_depth(id)
    }

    /// Calculates the maximum depth of any node within the subtree rooted at `id` (relative to `id`, root of subtree is 1).
    pub fn calculate_subtree_max_depth(&self, id: NodeId) -> usize {
        let mut max_child_depth = 0;
        if let Some(node) = self.nodes.get(&id) {
            for child_id in &node.ordered_children {
                let child_depth = self.calculate_subtree_max_depth(*child_id);
                if child_depth > max_child_depth {
                    max_child_depth = child_depth;
                }
            }
        }
        1 + max_child_depth
    }

    /// Creates a new node in the graph (§13 CREATE_NODE, §6.2, §26).
    pub fn create_node(
        &mut self,
        id: NodeId,
        node_type: TypeRef,
        parent_id: Option<NodeId>,
        child_index: Option<usize>,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Result<(), StoreError> {
        // 1. §6.2 Invariant: NodeId never reused in session
        if self.used_ids.contains(&id) {
            return Err(StoreError::NodeIdAlreadyUsed(id));
        }

        // 2. §26 Limit: max_node_count
        if self.nodes.len() >= self.limits.max_node_count {
            return Err(StoreError::MaxNodeCountExceeded {
                limit: self.limits.max_node_count,
                current: self.nodes.len(),
            });
        }

        // 3. Check parent existence and calculate depth (§26 max_tree_depth)
        let depth = match parent_id {
            Some(pid) => {
                let parent_depth = self
                    .calculate_node_depth(pid)
                    .ok_or(StoreError::ParentNotFound(pid))?;
                let target_depth = parent_depth + 1;
                if target_depth > self.limits.max_tree_depth {
                    return Err(StoreError::MaxTreeDepthExceeded {
                        limit: self.limits.max_tree_depth,
                        actual: target_depth,
                    });
                }
                target_depth
            }
            None => 1,
        };

        if depth > self.limits.max_tree_depth {
            return Err(StoreError::MaxTreeDepthExceeded {
                limit: self.limits.max_tree_depth,
                actual: depth,
            });
        }

        // 4. Validate all properties and limits
        let prop_list: Vec<(PropertyRef, Value)> = properties.into_iter().collect();
        for (_, val) in &prop_list {
            self.limits.validate_value(val)?;
        }

        // 5. Validate insertion index if parent specified
        if let Some(pid) = parent_id {
            let parent_node = self.nodes.get(&pid).unwrap();
            let child_count = parent_node.ordered_children.len();
            if let Some(idx) = child_index {
                if idx > child_count {
                    return Err(StoreError::ChildIndexOutOfBounds {
                        index: idx,
                        count: child_count,
                    });
                }
            }
        }

        // 6. Insert into parent's ordered_children or roots
        match parent_id {
            Some(pid) => {
                let parent_node = self.nodes.get_mut(&pid).unwrap();
                match child_index {
                    Some(idx) => parent_node.ordered_children.insert(idx, id),
                    None => parent_node.ordered_children.push(id),
                }
            }
            None => match child_index {
                Some(idx) => {
                    if idx > self.roots.len() {
                        return Err(StoreError::ChildIndexOutOfBounds {
                            index: idx,
                            count: self.roots.len(),
                        });
                    }
                    self.roots.insert(idx, id);
                }
                None => self.roots.push(id),
            },
        }

        // 7. Insert node and record used ID
        let node = Node::new(id, node_type, parent_id, prop_list);
        self.nodes.insert(id, node);
        self.used_ids.insert(id);

        Ok(())
    }

    /// Deletes a node and all of its descendants recursively, returning all deleted node IDs (§13 DELETE_NODE, §6.2).
    pub fn delete_node(&mut self, id: NodeId) -> Result<Vec<NodeId>, StoreError> {
        let node = self.nodes.get(&id).ok_or(StoreError::NodeNotFound(id))?;
        let parent_id = node.parent_id;

        // 1. Remove from parent's ordered_children or roots list
        match parent_id {
            Some(pid) => {
                if let Some(parent_node) = self.nodes.get_mut(&pid) {
                    parent_node.ordered_children.retain(|&child| child != id);
                }
            }
            None => {
                self.roots.retain(|&root| root != id);
            }
        }

        // 2. Recursively delete node and all descendants
        let mut deleted = Vec::new();
        self.delete_subtree_recursive(id, &mut deleted);
        Ok(deleted)
    }

    fn delete_subtree_recursive(&mut self, id: NodeId, deleted: &mut Vec<NodeId>) {
        if let Some(node) = self.nodes.remove(&id) {
            deleted.push(id);
            for child_id in node.ordered_children {
                self.delete_subtree_recursive(child_id, deleted);
            }
        }
    }

    /// Sets or updates a property on a node, returning the previous value if defined (§13 SET_PROPERTY, §26).
    pub fn set_property(
        &mut self,
        id: NodeId,
        prop: PropertyRef,
        val: Value,
    ) -> Result<Option<Value>, StoreError> {
        self.limits.validate_value(&val)?;
        let node = self.nodes.get_mut(&id).ok_or(StoreError::NodeNotFound(id))?;
        Ok(node.properties.insert(prop, val))
    }

    /// Clears a property from a node (§13 CLEAR_PROPERTY).
    pub fn clear_property(&mut self, id: NodeId, prop: PropertyRef) -> Result<Option<Value>, StoreError> {
        let node = self.nodes.get_mut(&id).ok_or(StoreError::NodeNotFound(id))?;
        Ok(node.properties.remove(&prop))
    }

    /// Sets multiple properties on a node atomically (§13 BATCH_PROPERTY_SET, §26).
    pub fn batch_property_set(
        &mut self,
        id: NodeId,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Result<(), StoreError> {
        let prop_list: Vec<(PropertyRef, Value)> = properties.into_iter().collect();
        for (_, val) in &prop_list {
            self.limits.validate_value(val)?;
        }

        let node = self.nodes.get_mut(&id).ok_or(StoreError::NodeNotFound(id))?;
        for (prop, val) in prop_list {
            node.properties.insert(prop, val);
        }
        Ok(())
    }

    /// Moves a node to a new parent and/or child index (§13 MOVE_NODE, §26).
    pub fn move_node(
        &mut self,
        id: NodeId,
        new_parent_id: Option<NodeId>,
        new_child_index: Option<usize>,
    ) -> Result<(), StoreError> {
        if !self.nodes.contains_key(&id) {
            return Err(StoreError::NodeNotFound(id));
        }

        // 1. Cycle detection: new_parent_id cannot be `id` or any descendant of `id`
        if let Some(target_parent) = new_parent_id {
            if target_parent == id {
                return Err(StoreError::CycleDetected {
                    node_id: id,
                    target_parent,
                });
            }
            let mut curr = target_parent;
            while let Some(parent_node) = self.nodes.get(&curr) {
                if let Some(pid) = parent_node.parent_id {
                    if pid == id {
                        return Err(StoreError::CycleDetected {
                            node_id: id,
                            target_parent,
                        });
                    }
                    curr = pid;
                } else {
                    break;
                }
            }
        }

        // 2. Tree depth limit validation (§26)
        let subtree_depth = self.calculate_subtree_max_depth(id);
        let new_parent_depth = match new_parent_id {
            Some(pid) => {
                self.calculate_node_depth(pid)
                    .ok_or(StoreError::ParentNotFound(pid))?
            }
            None => 0,
        };
        let new_total_depth = new_parent_depth + subtree_depth;
        if new_total_depth > self.limits.max_tree_depth {
            return Err(StoreError::MaxTreeDepthExceeded {
                limit: self.limits.max_tree_depth,
                actual: new_total_depth,
            });
        }

        let old_parent_id = self.nodes.get(&id).unwrap().parent_id;

        // 3. Validate new_child_index bounds against current destination container length
        let current_dest_len = match new_parent_id {
            Some(pid) => self.nodes.get(&pid).unwrap().ordered_children.len(),
            None => self.roots.len(),
        };

        if let Some(idx) = new_child_index {
            if idx > current_dest_len {
                return Err(StoreError::ChildIndexOutOfBounds {
                    index: idx,
                    count: current_dest_len,
                });
            }
        }

        // 4. Remove from old location
        match old_parent_id {
            Some(pid) => {
                if let Some(parent_node) = self.nodes.get_mut(&pid) {
                    parent_node.ordered_children.retain(|&c| c != id);
                }
            }
            None => {
                self.roots.retain(|&r| r != id);
            }
        }

        // 5. Insert into new location
        match new_parent_id {
            Some(pid) => {
                let parent_node = self.nodes.get_mut(&pid).unwrap();
                match new_child_index {
                    Some(idx) => {
                        let target_idx = idx.min(parent_node.ordered_children.len());
                        parent_node.ordered_children.insert(target_idx, id);
                    }
                    None => parent_node.ordered_children.push(id),
                }
            }
            None => match new_child_index {
                Some(idx) => {
                    let target_idx = idx.min(self.roots.len());
                    self.roots.insert(target_idx, id);
                }
                None => self.roots.push(id),
            },
        }

        // 6. Update node's parent_id
        let node = self.nodes.get_mut(&id).unwrap();
        node.parent_id = new_parent_id;

        Ok(())
    }

    /// Reorders the children of a parent node to match a given sequence (§13 REORDER_CHILDREN).
    pub fn reorder_children(&mut self, parent_id: NodeId, new_order: &[NodeId]) -> Result<(), StoreError> {
        let parent_node = self.nodes.get(&parent_id).ok_or(StoreError::ParentNotFound(parent_id))?;
        let current_children = &parent_node.ordered_children;

        if new_order.len() != current_children.len() {
            return Err(StoreError::InvalidChildrenReorder {
                parent_id,
                reason: format!(
                    "expected {} children, got {}",
                    current_children.len(),
                    new_order.len()
                ),
            });
        }

        let curr_set: HashSet<NodeId> = current_children.iter().copied().collect();
        let mut new_set: HashSet<NodeId> = HashSet::with_capacity(new_order.len());

        for &child in new_order {
            if !curr_set.contains(&child) {
                return Err(StoreError::InvalidChildrenReorder {
                    parent_id,
                    reason: format!("child {} is not a child of parent {}", child, parent_id),
                });
            }
            if !new_set.insert(child) {
                return Err(StoreError::InvalidChildrenReorder {
                    parent_id,
                    reason: format!("duplicate child {} in permutation list", child),
                });
            }
        }

        let parent_node_mut = self.nodes.get_mut(&parent_id).unwrap();
        parent_node_mut.ordered_children = new_order.to_vec();
        Ok(())
    }

    /// Returns the number of active models currently in the store (§8).
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Returns `true` if an active model exists with the given ID (§8).
    pub fn contains_model(&self, id: ModelId) -> bool {
        self.models.contains_key(&id)
    }

    /// Returns `true` if the given `ModelId` was ever used in this session (even if deleted, §6.2, §8).
    pub fn is_model_id_used(&self, id: ModelId) -> bool {
        self.used_model_ids.contains(&id)
    }

    /// Returns an immutable reference to the model with the given ID.
    pub fn get_model(&self, id: ModelId) -> Option<&Model> {
        self.models.get(&id)
    }

    /// Returns a mutable reference to the model with the given ID.
    pub fn get_model_mut(&mut self, id: ModelId) -> Option<&mut Model> {
        self.models.get_mut(&id)
    }

    /// Returns the model referenced by the given node, if any (§8).
    pub fn get_model_for_node(&self, node_id: NodeId) -> Option<&Model> {
        let node = self.nodes.get(&node_id)?;
        let model_id = node.model_ref()?;
        self.models.get(&model_id)
    }

    /// Creates a new collection model in the store (§13 CREATE_MODEL, §8).
    pub fn create_model(
        &mut self,
        id: ModelId,
        model_type: TypeRef,
        item_count: u64,
    ) -> Result<(), StoreError> {
        if self.used_model_ids.contains(&id) {
            return Err(StoreError::ModelIdAlreadyUsed(id));
        }

        let model = Model::new(id, model_type, item_count);
        self.models.insert(id, model);
        self.used_model_ids.insert(id);
        Ok(())
    }

    /// Deletes a model from the store, returning the deleted model if it existed.
    pub fn delete_model(&mut self, id: ModelId) -> Result<Option<Model>, StoreError> {
        if !self.models.contains_key(&id) {
            return Err(StoreError::ModelNotFound(id));
        }
        Ok(self.models.remove(&id))
    }

    /// Inserts items into a collection model at a specified index (§13 MODEL_INSERT).
    pub fn model_insert(
        &mut self,
        id: ModelId,
        index: u64,
        items: Vec<ModelItem>,
    ) -> Result<(), StoreError> {
        for item in &items {
            self.limits.validate_value(&item.value)?;
            for (_, val) in &item.properties {
                self.limits.validate_value(val)?;
            }
        }

        let model = self.models.get_mut(&id).ok_or(StoreError::ModelNotFound(id))?;
        model.insert_items(index, items)
    }

    /// Deletes items from a collection model by item identity and/or index range (§13 MODEL_DELETE).
    pub fn model_delete(
        &mut self,
        id: ModelId,
        index: Option<u64>,
        count: Option<u64>,
        item_ids: Vec<ItemId>,
    ) -> Result<(), StoreError> {
        let model = self.models.get_mut(&id).ok_or(StoreError::ModelNotFound(id))?;
        model.delete_items(index, count, &item_ids)
    }

    /// Updates existing items in a collection model (§13 MODEL_UPDATE).
    pub fn model_update(
        &mut self,
        id: ModelId,
        index: Option<u64>,
        items: Vec<ModelItem>,
    ) -> Result<(), StoreError> {
        for item in &items {
            self.limits.validate_value(&item.value)?;
            for (_, val) in &item.properties {
                self.limits.validate_value(val)?;
            }
        }

        let model = self.models.get_mut(&id).ok_or(StoreError::ModelNotFound(id))?;
        model.update_items(index, items)
    }

    /// Resets/replaces a range of cached items in a collection model (§13 MODEL_RESET_RANGE).
    pub fn model_reset_range(
        &mut self,
        id: ModelId,
        start_index: u64,
        items: Vec<ModelItem>,
        total_count: Option<u64>,
    ) -> Result<(), StoreError> {
        for item in &items {
            self.limits.validate_value(&item.value)?;
            for (_, val) in &item.properties {
                self.limits.validate_value(val)?;
            }
        }

        let model = self.models.get_mut(&id).ok_or(StoreError::ModelNotFound(id))?;
        model.reset_range(start_index, items, total_count)
    }

    /// Applies a protobuf wire `Operation` directly to the store (§13, §16).
    pub fn apply_operation(&mut self, op: &srui_protocol::Operation) -> Result<(), StoreError> {
        use srui_protocol::operation::Op;

        let op_kind = match op.op.as_ref() {
            Some(k) => k,
            None => return Err(StoreError::OperationError("empty operation payload".to_string())),
        };

        match op_kind {
            Op::CreateNode(op) => {
                let rec = op.node.as_ref().ok_or_else(|| {
                    StoreError::OperationError("missing NodeRecord in CreateNodeOp".to_string())
                })?;
                let id = NodeId::new(rec.node_id);
                let node_type = rec
                    .r#type
                    .map(TypeRef::from)
                    .ok_or_else(|| StoreError::OperationError("missing TypeRef".to_string()))?;
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
                for wire_prop in &rec.properties {
                    let prop = Property::try_from(wire_prop.clone())
                        .map_err(|e| StoreError::OperationError(e.to_string()))?;
                    properties.push((prop.property, prop.value));
                }

                self.create_node(id, node_type, parent_id, child_index, properties)?;
                Ok(())
            }
            Op::DeleteNode(op) => {
                self.delete_node(NodeId::new(op.node_id))?;
                Ok(())
            }
            Op::SetProperty(op) => {
                let id = NodeId::new(op.node_id);
                let prop_ref = op
                    .property
                    .map(PropertyRef::from)
                    .ok_or_else(|| StoreError::OperationError("missing PropertyRef".to_string()))?;
                let val = op
                    .value
                    .as_ref()
                    .map(|v| Value::try_from(v.clone()))
                    .transpose()
                    .map_err(|e| StoreError::OperationError(e.to_string()))?
                    .unwrap_or(Value::Null);

                self.set_property(id, prop_ref, val)?;
                Ok(())
            }
            Op::ClearProperty(op) => {
                let id = NodeId::new(op.node_id);
                let prop_ref = op
                    .property
                    .map(PropertyRef::from)
                    .ok_or_else(|| StoreError::OperationError("missing PropertyRef".to_string()))?;
                self.clear_property(id, prop_ref)?;
                Ok(())
            }
            Op::MoveNode(op) => {
                let id = NodeId::new(op.node_id);
                let new_parent_id = if op.new_parent_id == 0 {
                    None
                } else {
                    Some(NodeId::new(op.new_parent_id))
                };
                let new_child_index = if new_parent_id.is_none() || op.new_child_index == u32::MAX {
                    None
                } else {
                    Some(op.new_child_index as usize)
                };
                self.move_node(id, new_parent_id, new_child_index)?;
                Ok(())
            }
            Op::ReorderChildren(op) => {
                let parent_id = NodeId::new(op.parent_id);
                let child_ids: Vec<NodeId> = op
                    .child_node_ids
                    .iter()
                    .copied()
                    .map(NodeId::new)
                    .collect();
                self.reorder_children(parent_id, &child_ids)?;
                Ok(())
            }
            Op::BatchPropertySet(op) => {
                let id = NodeId::new(op.node_id);
                let mut properties = Vec::with_capacity(op.properties.len());
                for wire_prop in &op.properties {
                    let prop = Property::try_from(wire_prop.clone())
                        .map_err(|e| StoreError::OperationError(e.to_string()))?;
                    properties.push((prop.property, prop.value));
                }
                self.batch_property_set(id, properties)?;
                Ok(())
            }
            Op::CreateModel(op) => {
                let id = ModelId::new(op.model_id);
                let model_type = op
                    .model_type
                    .map(TypeRef::from)
                    .ok_or_else(|| StoreError::OperationError("missing TypeRef in CreateModelOp".to_string()))?;
                self.create_model(id, model_type, op.item_count)?;
                Ok(())
            }
            Op::ModelInsert(op) => {
                let id = ModelId::new(op.model_id);
                let mut items = Vec::with_capacity(op.items.len());
                for wire_item in &op.items {
                    let item = ModelItem::try_from(wire_item.clone())
                        .map_err(|e| StoreError::OperationError(e.to_string()))?;
                    items.push(item);
                }
                self.model_insert(id, op.index, items)?;
                Ok(())
            }
            Op::ModelDelete(op) => {
                let id = ModelId::new(op.model_id);
                let index = if op.count > 0 { Some(op.index) } else { None };
                let count = if op.count > 0 { Some(op.count) } else { None };
                let item_ids = op.item_ids.iter().copied().map(ItemId::new).collect();
                self.model_delete(id, index, count, item_ids)?;
                Ok(())
            }
            Op::ModelUpdate(op) => {
                let id = ModelId::new(op.model_id);
                let index = if op.index == u64::MAX { None } else { Some(op.index) };
                let mut items = Vec::with_capacity(op.items.len());
                for wire_item in &op.items {
                    let item = ModelItem::try_from(wire_item.clone())
                        .map_err(|e| StoreError::OperationError(e.to_string()))?;
                    items.push(item);
                }
                self.model_update(id, index, items)?;
                Ok(())
            }
            Op::ModelResetRange(op) => {
                let id = ModelId::new(op.model_id);
                let total_count = if op.total_count > 0 { Some(op.total_count) } else { None };
                let mut items = Vec::with_capacity(op.items.len());
                for wire_item in &op.items {
                    let item = ModelItem::try_from(wire_item.clone())
                        .map_err(|e| StoreError::OperationError(e.to_string()))?;
                    items.push(item);
                }
                self.model_reset_range(id, op.start_index, items, total_count)?;
                Ok(())
            }
            _ => Err(StoreError::OperationError(
                "unsupported or unhandled operation type".to_string(),
            )),
        }
    }
}
