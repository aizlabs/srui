//! Strongly-typed widget builders, handles, and property accessors (§7.1–§7.5).
//!
//! # Architecture & Design Invariants
//!
//! - **§7.1 Semantic Control, Local Appearance**: Widgets synchronize meaning and state, not paint instructions.
//! - **§7.2 Portable Control Vocabulary**: Implements exactly the required v0.1 tier (§7.3).
//! - **§7.2 Checkbox & Switch**: Single `Toggle` semantic state machine (`TypeRef::TOGGLE`) with `value: bool`
//!   and advisory `presentation_hint` enum. No separate "Checkbox" or "Switch" node types exist at the store level.
//! - **§7.4 Common Properties**: Typed getters, setters, and builder methods mapping directly to `PropertyRef`
//!   and `Value` variants on the underlying generic `SemanticStore`.
//! - **§7.5 Standard Appearance Roles**: Strongly-typed enums (`TextRole`, `ActionRole`, `InputRole`, `Importance`).
//! - **Thin Wrapper**: The typed layer retains zero divergent state; all mutations and reads route through
//!   `SemanticStore` / `Operation`.

use crate::StoreMut;

pub use srui_semantic_tree::{
    ItemId, ModelId, Node, NodeId, Operation, PropertyRef, ResourceHash, SemanticStore, Size,
    StoreError, TypeRef, Value,
};

// Re-export standard enums with canonical, ergonomic names (§7.4, §7.5)
pub use srui_semantic_tree::{
    StandardActionRole as ActionRole, StandardHorizontalAlignment as HorizontalAlignment,
    StandardImportance as Importance, StandardInputRole as InputRole,
    StandardPaddingRole as PaddingRole, StandardSelectionMode as SelectionMode,
    StandardSpacingRole as SpacingRole, StandardTextRole as TextRole,
    StandardTogglePresentationHint as TogglePresentationHint,
    StandardValidationState as ValidationState, StandardVerticalAlignment as VerticalAlignment,
    StandardVisibility as Visibility,
};

/// Trait implemented by all typed widget handles (§7.2, §7.3).
pub trait Widget: Copy + Clone + PartialEq + Eq + std::hash::Hash + std::fmt::Debug {
    /// The standard `TypeRef` for this widget type (§7.2).
    const NODE_TYPE: TypeRef;

    /// Returns the node ID of this widget instance (§6.2).
    fn id(&self) -> NodeId;

    /// Returns the node type of this widget instance (§6.4).
    fn node_type(&self) -> TypeRef {
        Self::NODE_TYPE
    }

    /// Verifies and wraps an existing node if its `node_type` matches `Self::NODE_TYPE`.
    fn from_node(node: &Node) -> Option<Self>
    where
        Self: Sized;

    /// Verifies and returns a typed handle if the node exists in the store and matches `Self::NODE_TYPE`.
    fn from_store(store: &SemanticStore, id: NodeId) -> Option<Self>
    where
        Self: Sized,
    {
        store.get_node(id).and_then(Self::from_node)
    }

    /// Deletes this widget and all its descendants from the store (§13 DELETE_NODE).
    fn delete(&self, store: &mut impl StoreMut) -> Result<Vec<NodeId>, StoreError> {
        store.delete_node(self.id())
    }

    /// Returns a [`Operation::DeleteNode`] operation for this widget (§13).
    fn op_delete(&self) -> Operation {
        Operation::delete_node(self.id())
    }
}

#[macro_use]
pub mod macros;

pub mod buttons;
pub mod collections;
pub mod inputs;
pub mod media;
pub mod primitives;
pub mod text;

/// Compatibility module re-exporting control widgets.
pub mod controls {
    pub use super::buttons::*;
    pub use super::inputs::*;
    pub use super::media::*;
    pub use super::text::*;
}

pub use buttons::*;
pub use collections::*;
pub use inputs::*;
pub use media::*;
pub use primitives::*;
pub use text::*;
