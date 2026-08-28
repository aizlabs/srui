use crate::ids::NodeId;
use std::fmt;

/// Errors returned by `SemanticStore` mutation operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// Attempted to create a node using a `NodeId` that was already used in this session (§6.2).
    NodeIdAlreadyUsed(NodeId),
    /// The specified node was not found in the store.
    NodeNotFound(NodeId),
    /// The specified parent node was not found in the store.
    ParentNotFound(NodeId),
    /// Reordering children received an invalid list of child IDs (must be an exact permutation).
    InvalidChildrenReorder { parent_id: NodeId, reason: String },
    /// Moving a node would create a parent-child cycle in the graph.
    CycleDetected { node_id: NodeId, target_parent: NodeId },
    /// Operation exceeds the configured maximum tree depth limit (§26).
    MaxTreeDepthExceeded { limit: usize, actual: usize },
    /// Operation exceeds the configured maximum node count limit (§26).
    MaxNodeCountExceeded { limit: usize, current: usize },
    /// Operation exceeds the configured maximum string length limit (§26).
    MaxStringLengthExceeded { limit: usize, actual: usize },
    /// Operation exceeds the configured maximum value nesting depth (§26).
    MaxValueDepthExceeded { limit: usize, actual: usize },
    /// Value list exceeds the configured maximum element count (§26).
    MaxListLengthExceeded { limit: usize, actual: usize },
    /// Small record exceeds the configured maximum properties count (§26).
    MaxRecordPropertiesExceeded { limit: usize, actual: usize },
    /// The specified child insertion index is out of bounds for the parent's current children list.
    ChildIndexOutOfBounds { index: usize, count: usize },
    /// Generic operation error when applying a wire operation.
    OperationError(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeIdAlreadyUsed(id) => write!(
                f,
                "NodeId {} has already been used in this session and cannot be reused (§6.2)",
                id
            ),
            Self::NodeNotFound(id) => write!(f, "node {} not found in store", id),
            Self::ParentNotFound(id) => write!(f, "parent node {} not found in store", id),
            Self::InvalidChildrenReorder { parent_id, reason } => write!(
                f,
                "invalid child permutation for parent {}: {}",
                parent_id, reason
            ),
            Self::CycleDetected {
                node_id,
                target_parent,
            } => write!(
                f,
                "cycle detected: moving node {} under {} creates an ancestor loop",
                node_id, target_parent
            ),
            Self::MaxTreeDepthExceeded { limit, actual } => write!(
                f,
                "tree depth limit exceeded: max allowed is {}, attempted depth is {}",
                limit, actual
            ),
            Self::MaxNodeCountExceeded { limit, current } => write!(
                f,
                "node count limit exceeded: max allowed is {}, current count is {}",
                limit, current
            ),
            Self::MaxStringLengthExceeded { limit, actual } => write!(
                f,
                "string length limit exceeded: max allowed is {} bytes, actual length is {}",
                limit, actual
            ),
            Self::MaxValueDepthExceeded { limit, actual } => write!(
                f,
                "value nesting depth limit exceeded: max allowed is {}, actual depth is {}",
                limit, actual
            ),
            Self::MaxListLengthExceeded { limit, actual } => write!(
                f,
                "list length limit exceeded: max allowed is {} elements, actual length is {}",
                limit, actual
            ),
            Self::MaxRecordPropertiesExceeded { limit, actual } => write!(
                f,
                "record properties limit exceeded: max allowed is {}, actual count is {}",
                limit, actual
            ),
            Self::ChildIndexOutOfBounds { index, count } => write!(
                f,
                "child index {} out of bounds (current child count: {})",
                index, count
            ),
            Self::OperationError(msg) => write!(f, "operation application error: {}", msg),
        }
    }
}

impl std::error::Error for StoreError {}
