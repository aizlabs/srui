//! In-memory semantic store and mutation operations.
//!
//! Conforms to SRUI Specification v0.4:
//! - §6.2: Node identity rules (session-scoped stable `NodeId`, no reuse even after deletion)
//! - §6.3: State ownership (authoritative semantic store)
//! - §13: Core mutation operations (`CREATE_NODE`, `DELETE_NODE`, `SET_PROPERTY`, `CLEAR_PROPERTY`, `MOVE_NODE`, `REORDER_CHILDREN`, `BATCH_PROPERTY_SET`)
//! - §26: Mandatory limits (configurable max tree depth, max node count, max string length)

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ids::{NodeId, PropertyRef, TypeRef};
use crate::value::{Property, Value};

/// Default maximum tree depth (§26).
pub const DEFAULT_MAX_TREE_DEPTH: usize = 64;

/// Default maximum active node count in the store (§26).
pub const DEFAULT_MAX_NODE_COUNT: usize = 100_000;

/// Default maximum length in bytes for any UTF-8 string property value (§26).
pub const DEFAULT_MAX_STRING_LENGTH: usize = 1_048_576; // 1 MiB

/// Configurable limits for the semantic store (§26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreLimits {
    /// Maximum allowable tree depth from root (depth 1) to any descendant.
    pub max_tree_depth: usize,
    /// Maximum number of active nodes in the store.
    pub max_node_count: usize,
    /// Maximum length in bytes for any UTF-8 string property value.
    pub max_string_length: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            max_tree_depth: DEFAULT_MAX_TREE_DEPTH,
            max_node_count: DEFAULT_MAX_NODE_COUNT,
            max_string_length: DEFAULT_MAX_STRING_LENGTH,
        }
    }
}

impl StoreLimits {
    /// Creates a custom `StoreLimits` configuration.
    pub const fn new(
        max_tree_depth: usize,
        max_node_count: usize,
        max_string_length: usize,
    ) -> Self {
        Self {
            max_tree_depth,
            max_node_count,
            max_string_length,
        }
    }
}

/// Errors returned by [`SemanticStore`] mutation and query operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The specified `NodeId` has already been used in this session (even if previously deleted per §6.2).
    NodeIdAlreadyUsed(NodeId),
    /// The specified node was not found in the active store.
    NodeNotFound(NodeId),
    /// The specified parent node does not exist in the active store.
    ParentNotFound(NodeId),
    /// The operation would exceed the configured maximum tree depth (§26).
    MaxTreeDepthExceeded { limit: usize, actual: usize },
    /// The operation would exceed the configured maximum active node count (§26).
    MaxNodeCountExceeded { limit: usize, current: usize },
    /// A string value exceeds the configured maximum string length in bytes (§26).
    MaxStringLengthExceeded { limit: usize, actual: usize },
    /// Moving a node would create a cycle (e.g. moving a node under itself or its descendant).
    CycleDetected {
        node_id: NodeId,
        target_parent: NodeId,
    },
    /// An invalid child index was specified for child insertion or movement.
    InvalidChildIndex { index: usize, len: usize },
    /// Reordering children failed because the provided list does not match current children.
    InvalidChildrenReorder { parent_id: NodeId, reason: String },
    /// Protocol or conversion error when applying a wire operation.
    OperationError(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeIdAlreadyUsed(id) => write!(
                f,
                "node_id {} has already been used in this session and cannot be reused (§6.2)",
                id
            ),
            Self::NodeNotFound(id) => write!(f, "node {} not found in semantic store", id),
            Self::ParentNotFound(id) => {
                write!(f, "parent node {} not found in semantic store", id)
            }
            Self::MaxTreeDepthExceeded { limit, actual } => write!(
                f,
                "tree depth limit exceeded: max allowed is {}, actual depth would be {}",
                limit, actual
            ),
            Self::MaxNodeCountExceeded { limit, current } => write!(
                f,
                "active node count limit exceeded: max allowed is {}, current count is {}",
                limit, current
            ),
            Self::MaxStringLengthExceeded { limit, actual } => write!(
                f,
                "string length limit exceeded: max allowed is {} bytes, actual length is {} bytes",
                limit, actual
            ),
            Self::CycleDetected {
                node_id,
                target_parent,
            } => write!(
                f,
                "cannot move node {} under target parent {}: cycle detected",
                node_id, target_parent
            ),
            Self::InvalidChildIndex { index, len } => write!(
                f,
                "invalid child index {}: parent currently has {} children",
                index, len
            ),
            Self::InvalidChildrenReorder { parent_id, reason } => write!(
                f,
                "invalid child reordering for parent node {}: {}",
                parent_id, reason
            ),
            Self::OperationError(msg) => write!(f, "operation error: {}", msg),
        }
    }
}

impl std::error::Error for StoreError {}

/// An in-memory semantic node (§6.2).
///
/// Each semantic node has:
/// - `id`: Session-scoped unique [`NodeId`]
/// - `node_type`: [`TypeRef`]
/// - `parent_id`: `Option<NodeId>` (`None` for top-level / root surfaces)
/// - `ordered_children`: Ordered list of child [`NodeId`]s
/// - `properties`: Map of [`PropertyRef`] to [`Value`]
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Unique session-scoped node identifier (§6.2).
    pub id: NodeId,
    /// Type of this semantic node (§6.4, §7.2).
    pub node_type: TypeRef,
    /// Parent node ID, or `None` if this is a top-level surface / root.
    pub parent_id: Option<NodeId>,
    /// Ordered child node IDs.
    pub ordered_children: Vec<NodeId>,
    /// Property map for this node.
    pub properties: HashMap<PropertyRef, Value>,
}

impl Node {
    /// Creates a new `Node` without children or properties.
    pub fn new(id: NodeId, node_type: TypeRef, parent_id: Option<NodeId>) -> Self {
        Self {
            id,
            node_type,
            parent_id,
            ordered_children: Vec::new(),
            properties: HashMap::new(),
        }
    }

    /// Returns a reference to the property value if present.
    pub fn get_property(&self, prop: PropertyRef) -> Option<&Value> {
        self.properties.get(&prop)
    }

    /// Sets a property on the node, returning the previous value if present.
    pub fn set_property(&mut self, prop: PropertyRef, value: Value) -> Option<Value> {
        self.properties.insert(prop, value)
    }

    /// Clears a property from the node, returning the removed value if present.
    pub fn clear_property(&mut self, prop: PropertyRef) -> Option<Value> {
        self.properties.remove(&prop)
    }

    /// Returns the slice of ordered child IDs.
    pub fn children(&self) -> &[NodeId] {
        &self.ordered_children
    }
}

/// The authoritative in-memory semantic UI node graph store (§6.2, §6.3).
///
/// Maintains a persistent object graph updated by direct mutation operations (§13)
/// and enforces structural invariants (§6.2, §26).
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticStore {
    /// Active nodes in the store: `node_id -> Node`.
    nodes: HashMap<NodeId, Node>,
    /// Set of all `NodeId`s that have ever been created in this store's lifetime (§6.2).
    ///
    /// Even after `delete_node`, deleted IDs remain recorded here and can never be reused.
    used_node_ids: HashSet<NodeId>,
    /// Configurable limits (§26).
    limits: StoreLimits,
}

impl Default for SemanticStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticStore {
    /// Creates a new empty `SemanticStore` with default limits (§26).
    pub fn new() -> Self {
        Self::with_limits(StoreLimits::default())
    }

    /// Creates a new empty `SemanticStore` with custom limits.
    pub fn with_limits(limits: StoreLimits) -> Self {
        Self {
            nodes: HashMap::new(),
            used_node_ids: HashSet::new(),
            limits,
        }
    }

    /// Returns a reference to the store limits configuration.
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

    /// Returns `true` if the active store contains a node with the given `id`.
    pub fn contains_node(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Returns `true` if the given `id` has ever been used in this store's lifetime (§6.2).
    pub fn is_id_used(&self, id: NodeId) -> bool {
        self.used_node_ids.contains(&id)
    }

    /// Returns a reference to the active node with the given `id`, if present.
    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Returns an iterator over all active `(NodeId, &Node)` pairs in the store.
    pub fn iter_nodes(&self) -> impl Iterator<Item = (&NodeId, &Node)> {
        self.nodes.iter()
    }

    /// Returns the list of all root node IDs (nodes where `parent_id` is `None`).
    pub fn root_ids(&self) -> Vec<NodeId> {
        self.nodes
            .values()
            .filter(|n| n.parent_id.is_none())
            .map(|n| n.id)
            .collect()
    }

    /// Returns the depth of the specified node (root nodes have depth 1).
    pub fn node_depth(&self, id: NodeId) -> Option<usize> {
        let mut current = id;
        let mut depth = 1;
        while let Some(node) = self.nodes.get(&current) {
            match node.parent_id {
                Some(parent_id) => {
                    current = parent_id;
                    depth += 1;
                }
                None => return Some(depth),
            }
        }
        None
    }

    /// Returns the maximum subtree depth rooted at `id` (a leaf node has subtree depth 1).
    pub fn subtree_depth(&self, id: NodeId) -> usize {
        if let Some(node) = self.nodes.get(&id) {
            let mut max_child_depth = 0;
            for child_id in &node.ordered_children {
                max_child_depth = max_child_depth.max(self.subtree_depth(*child_id));
            }
            1 + max_child_depth
        } else {
            0
        }
    }

    /// Returns the ordered children of the specified node, if it exists.
    pub fn children_of(&self, id: NodeId) -> Option<&[NodeId]> {
        self.nodes.get(&id).map(|n| n.ordered_children.as_slice())
    }

    /// Returns the parent ID of the specified node, if it exists.
    pub fn parent_of(&self, id: NodeId) -> Option<Option<NodeId>> {
        self.nodes.get(&id).map(|n| n.parent_id)
    }

    /// Returns all descendant node IDs of `id` in pre-order traversal.
    pub fn all_descendants(&self, id: NodeId) -> Option<Vec<NodeId>> {
        if !self.nodes.contains_key(&id) {
            return None;
        }
        let mut descendants = Vec::new();
        let mut stack = Vec::new();
        if let Some(node) = self.nodes.get(&id) {
            for child_id in node.ordered_children.iter().rev() {
                stack.push(*child_id);
            }
        }
        while let Some(curr_id) = stack.pop() {
            descendants.push(curr_id);
            if let Some(curr_node) = self.nodes.get(&curr_id) {
                for child_id in curr_node.ordered_children.iter().rev() {
                    stack.push(*child_id);
                }
            }
        }
        Some(descendants)
    }

    // -------------------------------------------------------------------------
    // Direct Mutation Primitives (§13)
    // -------------------------------------------------------------------------

    /// Creates a new semantic node in the store (`CREATE_NODE`, §13).
    ///
    /// # Invariants & Limits Enforced:
    /// - `id` must not have been used previously in this session (§6.2), returning `NodeIdAlreadyUsed`.
    /// - `parent_id` (if `Some`) must exist in the active store, returning `ParentNotFound`.
    /// - Node count must not exceed `limits.max_node_count` (§26), returning `MaxNodeCountExceeded`.
    /// - Tree depth must not exceed `limits.max_tree_depth` (§26), returning `MaxTreeDepthExceeded`.
    /// - `child_index` (if `Some`) must be `<= parent.ordered_children.len()`, returning `InvalidChildIndex`.
    /// - String property values must not exceed `limits.max_string_length` (§26), returning `MaxStringLengthExceeded`.
    ///
    /// If any check fails, the store is left completely unchanged.
    pub fn create_node(
        &mut self,
        id: NodeId,
        node_type: TypeRef,
        parent_id: Option<NodeId>,
        child_index: Option<usize>,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Result<&Node, StoreError> {
        // 1. Verify Node ID uniqueness across session lifetime (§6.2).
        if self.used_node_ids.contains(&id) {
            return Err(StoreError::NodeIdAlreadyUsed(id));
        }

        // 2. Verify active node count limit (§26).
        if self.nodes.len() >= self.limits.max_node_count {
            return Err(StoreError::MaxNodeCountExceeded {
                limit: self.limits.max_node_count,
                current: self.nodes.len(),
            });
        }

        // 3. Verify parent existence, tree depth limit (§26), and child index.
        let target_depth = match parent_id {
            Some(pid) => {
                let parent = self.nodes.get(&pid).ok_or(StoreError::ParentNotFound(pid))?;
                if let Some(idx) = child_index {
                    if idx > parent.ordered_children.len() {
                        return Err(StoreError::InvalidChildIndex {
                            index: idx,
                            len: parent.ordered_children.len(),
                        });
                    }
                }
                let parent_depth = self
                    .node_depth(pid)
                    .ok_or(StoreError::ParentNotFound(pid))?;
                parent_depth + 1
            }
            None => 1,
        };

        if target_depth > self.limits.max_tree_depth {
            return Err(StoreError::MaxTreeDepthExceeded {
                limit: self.limits.max_tree_depth,
                actual: target_depth,
            });
        }

        // 4. Collect and validate properties against string length limit (§26).
        let mut prop_map = HashMap::new();
        for (prop_ref, val) in properties {
            self.validate_value(&val)?;
            prop_map.insert(prop_ref, val);
        }

        // 5. All validation succeeded; apply mutations atomically to store.
        self.used_node_ids.insert(id);

        if let Some(pid) = parent_id {
            let parent = self
                .nodes
                .get_mut(&pid)
                .expect("parent verified to exist above");
            if let Some(idx) = child_index {
                parent.ordered_children.insert(idx, id);
            } else {
                parent.ordered_children.push(id);
            }
        }

        let node = Node {
            id,
            node_type,
            parent_id,
            ordered_children: Vec::new(),
            properties: prop_map,
        };

        self.nodes.insert(id, node);
        Ok(self.nodes.get(&id).expect("node inserted"))
    }

    /// Deletes a node and all of its descendants recursively (`DELETE_NODE`, §13).
    ///
    /// # Invariants & Rules:
    /// - Node `id` must exist in active store, returning `NodeNotFound`.
    /// - Detaches `id` from its parent's `ordered_children`.
    /// - Recursively removes the deleted node and all descendant nodes from active `nodes`.
    /// - Retains all deleted IDs in `used_node_ids` so they can never be reused (§6.2).
    /// - Returns the list of all deleted `NodeId`s.
    pub fn delete_node(&mut self, id: NodeId) -> Result<Vec<NodeId>, StoreError> {
        let node = self.nodes.get(&id).ok_or(StoreError::NodeNotFound(id))?;
        let parent_id = node.parent_id;

        // 1. Detach from parent's ordered_children.
        if let Some(pid) = parent_id {
            if let Some(parent) = self.nodes.get_mut(&pid) {
                parent.ordered_children.retain(|&child| child != id);
            }
        }

        // 2. Recursively collect all descendant IDs.
        let mut to_delete = Vec::new();
        let mut stack = vec![id];
        while let Some(curr_id) = stack.pop() {
            to_delete.push(curr_id);
            if let Some(curr_node) = self.nodes.get(&curr_id) {
                for child_id in &curr_node.ordered_children {
                    stack.push(*child_id);
                }
            }
        }

        // 3. Remove all collected nodes from active nodes map.
        for del_id in &to_delete {
            self.nodes.remove(del_id);
        }

        Ok(to_delete)
    }

    /// Sets a property on an active node (`SET_PROPERTY`, §13).
    ///
    /// Returns the previous property value if present.
    /// Returns `NodeNotFound` if the node does not exist, or `MaxStringLengthExceeded` if limit is exceeded.
    pub fn set_property(
        &mut self,
        id: NodeId,
        property: PropertyRef,
        value: Value,
    ) -> Result<Option<Value>, StoreError> {
        if !self.nodes.contains_key(&id) {
            return Err(StoreError::NodeNotFound(id));
        }
        self.validate_value(&value)?;
        let node = self.nodes.get_mut(&id).expect("node existence verified");
        Ok(node.set_property(property, value))
    }

    /// Clears a property from an active node (`CLEAR_PROPERTY`, §13).
    ///
    /// Returns the removed property value if present, or `None` if it was not set.
    /// Returns `NodeNotFound` if the node does not exist.
    pub fn clear_property(
        &mut self,
        id: NodeId,
        property: PropertyRef,
    ) -> Result<Option<Value>, StoreError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(StoreError::NodeNotFound(id))?;
        Ok(node.clear_property(property))
    }

    /// Moves a node under a new parent at the specified child index (`MOVE_NODE`, §13).
    ///
    /// # Invariants & Limits Enforced:
    /// - `id` must exist in active store, returning `NodeNotFound`.
    /// - `new_parent_id` (if `Some`) must exist in active store, returning `ParentNotFound`.
    /// - Moving a node under itself or its descendant is rejected with `CycleDetected`.
    /// - Resulting tree depth of any descendant must not exceed `limits.max_tree_depth` (§26).
    /// - `new_child_index` (if `Some`) must be within bounds for the new parent's children.
    pub fn move_node(
        &mut self,
        id: NodeId,
        new_parent_id: Option<NodeId>,
        new_child_index: Option<usize>,
    ) -> Result<(), StoreError> {
        let old_parent_id = self
            .nodes
            .get(&id)
            .ok_or(StoreError::NodeNotFound(id))?
            .parent_id;

        // 1. Validate target parent and cycle detection.
        if let Some(new_pid) = new_parent_id {
            let new_parent = self
                .nodes
                .get(&new_pid)
                .ok_or(StoreError::ParentNotFound(new_pid))?;

            // Cycle detection: new_pid cannot be `id` or any descendant of `id`.
            let mut curr = Some(new_pid);
            while let Some(pid) = curr {
                if pid == id {
                    return Err(StoreError::CycleDetected {
                        node_id: id,
                        target_parent: new_pid,
                    });
                }
                curr = self.nodes.get(&pid).and_then(|n| n.parent_id);
            }

            // Tree depth calculation after move.
            let parent_depth = self
                .node_depth(new_pid)
                .ok_or(StoreError::ParentNotFound(new_pid))?;
            let subtree_h = self.subtree_depth(id);
            let max_result_depth = parent_depth + subtree_h;

            if max_result_depth > self.limits.max_tree_depth {
                return Err(StoreError::MaxTreeDepthExceeded {
                    limit: self.limits.max_tree_depth,
                    actual: max_result_depth,
                });
            }

            // Child index validation.
            if let Some(idx) = new_child_index {
                let effective_len = if old_parent_id == Some(new_pid) {
                    new_parent.ordered_children.len()
                } else {
                    new_parent.ordered_children.len() + 1
                };
                if idx >= effective_len {
                    return Err(StoreError::InvalidChildIndex {
                        index: idx,
                        len: new_parent.ordered_children.len(),
                    });
                }
            }
        } else {
            // Moving to root (new_parent_id == None).
            let subtree_h = self.subtree_depth(id);
            if subtree_h > self.limits.max_tree_depth {
                return Err(StoreError::MaxTreeDepthExceeded {
                    limit: self.limits.max_tree_depth,
                    actual: subtree_h,
                });
            }
        }

        // 2. Apply move.
        // Detach from old parent.
        if let Some(old_pid) = old_parent_id {
            if let Some(old_parent) = self.nodes.get_mut(&old_pid) {
                old_parent.ordered_children.retain(|&child| child != id);
            }
        }

        // Attach to new parent.
        if let Some(new_pid) = new_parent_id {
            let new_parent = self
                .nodes
                .get_mut(&new_pid)
                .expect("new parent existence verified");
            if let Some(idx) = new_child_index {
                let insert_idx = idx.min(new_parent.ordered_children.len());
                new_parent.ordered_children.insert(insert_idx, id);
            } else {
                new_parent.ordered_children.push(id);
            }
        }

        // Update node's parent_id.
        let node = self.nodes.get_mut(&id).expect("node existence verified");
        node.parent_id = new_parent_id;

        Ok(())
    }

    /// Reorders the children of an active node (`REORDER_CHILDREN`, §13).
    ///
    /// # Invariants:
    /// - `parent_id` must exist in active store, returning `NodeNotFound`.
    /// - `child_node_ids` must be an exact permutation of current children: same length,
    ///   containing all current children with no duplicates and no foreign nodes.
    pub fn reorder_children(
        &mut self,
        parent_id: NodeId,
        child_node_ids: &[NodeId],
    ) -> Result<(), StoreError> {
        let parent = self
            .nodes
            .get(&parent_id)
            .ok_or(StoreError::NodeNotFound(parent_id))?;

        if child_node_ids.len() != parent.ordered_children.len() {
            return Err(StoreError::InvalidChildrenReorder {
                parent_id,
                reason: format!(
                    "child count mismatch: expected {}, got {}",
                    parent.ordered_children.len(),
                    child_node_ids.len()
                ),
            });
        }

        let current_set: HashSet<NodeId> = parent.ordered_children.iter().copied().collect();
        let mut seen = HashSet::with_capacity(child_node_ids.len());

        for &child_id in child_node_ids {
            if !current_set.contains(&child_id) {
                return Err(StoreError::InvalidChildrenReorder {
                    parent_id,
                    reason: format!(
                        "node {} is not currently a child of parent {}",
                        child_id, parent_id
                    ),
                });
            }
            if !seen.insert(child_id) {
                return Err(StoreError::InvalidChildrenReorder {
                    parent_id,
                    reason: format!("duplicate child node {} in reorder list", child_id),
                });
            }
        }

        let parent_mut = self
            .nodes
            .get_mut(&parent_id)
            .expect("parent existence verified");
        parent_mut.ordered_children = child_node_ids.to_vec();

        Ok(())
    }

    /// Sets multiple properties on an active node (`BATCH_PROPERTY_SET`, §13).
    pub fn batch_property_set(
        &mut self,
        id: NodeId,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Result<(), StoreError> {
        if !self.nodes.contains_key(&id) {
            return Err(StoreError::NodeNotFound(id));
        }

        let collected: Vec<(PropertyRef, Value)> = properties.into_iter().collect();
        for (_, val) in &collected {
            self.validate_value(val)?;
        }

        let node = self.nodes.get_mut(&id).expect("node existence verified");
        for (prop, val) in collected {
            node.set_property(prop, val);
        }

        Ok(())
    }

    /// Applies a single protobuf wire operation directly to the semantic store (§13).
    pub fn apply_operation(&mut self, op: &srui_protocol::Operation) -> Result<(), StoreError> {
        use srui_protocol::operation::Op;

        let Some(op_kind) = &op.op else {
            return Err(StoreError::OperationError(
                "empty operation payload".to_string(),
            ));
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
                let child_index = Some(rec.child_index as usize);

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
                let new_child_index = Some(op.new_child_index as usize);
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
            other => Err(StoreError::OperationError(format!(
                "operation {:?} not handled at generic store primitive level",
                std::mem::discriminant(other)
            ))),
        }
    }

    // -------------------------------------------------------------------------
    // Internal Validation Helpers
    // -------------------------------------------------------------------------

    fn validate_value(&self, value: &Value) -> Result<(), StoreError> {
        match value {
            Value::String(s) => {
                if s.len() > self.limits.max_string_length {
                    return Err(StoreError::MaxStringLengthExceeded {
                        limit: self.limits.max_string_length,
                        actual: s.len(),
                    });
                }
            }
            Value::List(list) => {
                for item in list {
                    self.validate_value(item)?;
                }
            }
            Value::Record(record) => {
                for prop in &record.properties {
                    self.validate_value(&prop.value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{PropertyRef, TypeRef};

    #[test]
    fn test_create_and_query_nodes() {
        let mut store = SemanticStore::new();
        assert!(store.is_empty());

        let root_id = NodeId::new(1);
        let root = store
            .create_node(
                root_id,
                TypeRef::SURFACE,
                None,
                None,
                [(PropertyRef::LABEL, Value::from("Main Surface"))],
            )
            .expect("create root");

        assert_eq!(root.id, root_id);
        assert_eq!(root.node_type, TypeRef::SURFACE);
        assert_eq!(root.parent_id, None);
        assert_eq!(
            root.get_property(PropertyRef::LABEL),
            Some(&Value::from("Main Surface"))
        );
        assert_eq!(store.node_count(), 1);
        assert_eq!(store.root_ids(), vec![root_id]);
        assert_eq!(store.node_depth(root_id), Some(1));
        assert_eq!(store.subtree_depth(root_id), 1);

        let child_id = NodeId::new(2);
        store
            .create_node(
                child_id,
                TypeRef::COLUMN,
                Some(root_id),
                None,
                [(PropertyRef::SPACING_ROLE, Value::from(3u32))],
            )
            .expect("create child");

        assert_eq!(store.node_count(), 2);
        assert_eq!(store.children_of(root_id), Some(&[child_id][..]));
        assert_eq!(store.parent_of(child_id), Some(Some(root_id)));
        assert_eq!(store.node_depth(child_id), Some(2));
        assert_eq!(store.subtree_depth(root_id), 2);
    }
}
