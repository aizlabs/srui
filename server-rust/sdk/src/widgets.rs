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

use srui_semantic_tree::EnumToken;

pub use srui_semantic_tree::{
    ItemId, ModelId, Node, NodeId, Operation, PropertyRef, ResourceHash,
    SemanticStore, Size, StoreError, TypeRef, Value,
};

// Re-export standard enums with canonical, ergonomic names (§7.4, §7.5)
pub use srui_semantic_tree::{
    StandardActionRole as ActionRole,
    StandardHorizontalAlignment as HorizontalAlignment,
    StandardImportance as Importance,
    StandardInputRole as InputRole,
    StandardPaddingRole as PaddingRole,
    StandardSelectionMode as SelectionMode,
    StandardSpacingRole as SpacingRole,
    StandardTextRole as TextRole,
    StandardTogglePresentationHint as TogglePresentationHint,
    StandardValidationState as ValidationState,
    StandardVerticalAlignment as VerticalAlignment,
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
    fn delete(&self, store: &mut SemanticStore) -> Result<Vec<NodeId>, StoreError> {
        store.delete_node(self.id())
    }

    /// Returns a [`Operation::DeleteNode`] operation for this widget (§13).
    fn op_delete(&self) -> Operation {
        Operation::delete_node(self.id())
    }
}

// =============================================================================
// Helper Macros for Common Properties
// =============================================================================

macro_rules! impl_widget_boilerplate {
    ($widget:ident, $builder:ident, $type_ref:expr, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $widget {
            pub id: NodeId,
        }

        impl $widget {
            pub const NODE_TYPE: TypeRef = $type_ref;

            #[inline]
            pub const fn new(id: NodeId) -> Self {
                Self { id }
            }

            #[inline]
            pub fn builder(id: impl Into<NodeId>) -> $builder {
                $builder::new(id)
            }

            #[inline]
            pub fn id(&self) -> NodeId {
                self.id
            }

            #[inline]
            pub fn from_node(node: &Node) -> Option<Self> {
                if node.node_type == Self::NODE_TYPE {
                    Some(Self::new(node.id))
                } else {
                    None
                }
            }

            #[inline]
            pub fn from_store(store: &SemanticStore, id: NodeId) -> Option<Self> {
                store.get_node(id).and_then(Self::from_node)
            }
        }

        impl Widget for $widget {
            const NODE_TYPE: TypeRef = $type_ref;

            #[inline]
            fn id(&self) -> NodeId {
                self.id
            }

            #[inline]
            fn from_node(node: &Node) -> Option<Self> {
                Self::from_node(node)
            }
        }

        impl From<NodeId> for $widget {
            #[inline]
            fn from(id: NodeId) -> Self {
                Self::new(id)
            }
        }

        impl From<$widget> for NodeId {
            #[inline]
            fn from(w: $widget) -> Self {
                w.id
            }
        }

        #[doc = concat!("Builder for constructing [`", stringify!($widget), "`] nodes (§13 CREATE_NODE).")]
        #[derive(Debug, Clone)]
        pub struct $builder {
            id: NodeId,
            parent_id: Option<NodeId>,
            child_index: Option<usize>,
            properties: Vec<(PropertyRef, Value)>,
        }

        impl $builder {
            #[inline]
            pub fn new(id: impl Into<NodeId>) -> Self {
                Self {
                    id: id.into(),
                    parent_id: None,
                    child_index: None,
                    properties: Vec::new(),
                }
            }

            #[inline]
            #[must_use]
            pub fn parent(mut self, parent_id: impl Into<NodeId>) -> Self {
                self.parent_id = Some(parent_id.into());
                self
            }

            #[inline]
            #[must_use]
            pub fn child_index(mut self, index: usize) -> Self {
                self.child_index = Some(index);
                self
            }

            #[inline]
            #[must_use]
            pub fn property(mut self, prop: PropertyRef, val: impl Into<Value>) -> Self {
                self.properties.push((prop, val.into()));
                self
            }

            #[inline]
            pub fn into_operation(self) -> Operation {
                Operation::create_node(
                    self.id,
                    $widget::NODE_TYPE,
                    self.parent_id,
                    self.child_index,
                    self.properties,
                )
            }

            #[inline]
            pub fn create(self, store: &mut SemanticStore) -> Result<$widget, StoreError> {
                store.create_node(
                    self.id,
                    $widget::NODE_TYPE,
                    self.parent_id,
                    self.child_index,
                    self.properties,
                )?;
                Ok($widget::new(self.id))
            }
        }
    };
}

// -----------------------------------------------------------------------------
// Common Property Macros for Widget Handles & Builders
// -----------------------------------------------------------------------------

macro_rules! impl_string_prop {
    ($widget:ident, $builder:ident, $getter:ident, $getter_of:ident, $get_getter:ident, $setter:ident, $setter_for:ident, $op_set:ident, $op_set_for:ident, $clear:ident, $clear_for:ident, $op_clear:ident, $op_clear_for:ident, $prop_ref:expr, $doc:expr) => {
        impl $widget {
            #[doc = concat!("Returns the `", stringify!($getter), "` property if set.")]
            #[inline]
            #[allow(clippy::needless_lifetimes)]
            pub fn $getter<'a>(&self, store: &'a SemanticStore) -> Option<&'a str> {
                store.get_node(self.id).and_then(|n| Self::$getter_of(n))
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` property from a [`Node`].")]
            #[inline]
            pub fn $getter_of(node: &Node) -> Option<&str> {
                node.get_property($prop_ref).and_then(Value::as_string)
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` property for node `id` from `store`.")]
            #[inline]
            pub fn $get_getter(store: &SemanticStore, id: NodeId) -> Option<&str> {
                store.get_node(id).and_then(Self::$getter_of)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on this widget in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter(&self, store: &mut SemanticStore, value: impl Into<String>) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut SemanticStore, id: NodeId, value: impl Into<String>) -> Result<Option<Value>, StoreError> {
                store.set_property(id, $prop_ref, Value::String(value.into()))
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_set(&self, value: impl Into<String>) -> Operation {
                Self::$op_set_for(self.id, value)
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_set_for(id: NodeId, value: impl Into<String>) -> Operation {
                Operation::set_property(id, $prop_ref, Value::String(value.into()))
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on this widget in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
                store.clear_property(id, $prop_ref)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_clear(&self) -> Operation {
                Self::$op_clear_for(self.id)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_clear_for(id: NodeId) -> Operation {
                Operation::clear_property(id, $prop_ref)
            }
        }

        impl $builder {
            #[doc = concat!("Sets the `", stringify!($getter), "` property for the new widget (§7.4).")]
            #[inline]
            #[must_use]
            pub fn $getter(mut self, value: impl Into<String>) -> Self {
                self.properties.push(($prop_ref, Value::String(value.into())));
                self
            }
        }
    };
}

macro_rules! impl_bool_prop {
    ($widget:ident, $builder:ident, $getter:ident, $getter_of:ident, $get_getter:ident, $is_getter:ident, $is_getter_of:ident, $is_getter_for:ident, $setter:ident, $setter_for:ident, $op_set:ident, $op_set_for:ident, $clear:ident, $clear_for:ident, $op_clear:ident, $op_clear_for:ident, $prop_ref:expr, $doc:expr) => {
        impl $widget {
            #[doc = concat!("Returns the `", stringify!($getter), "` boolean property if set.")]
            #[inline]
            pub fn $getter(&self, store: &SemanticStore) -> Option<bool> {
                store.get_node(self.id).and_then(|n| Self::$getter_of(n))
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` boolean property from a [`Node`].")]
            #[inline]
            pub fn $getter_of(node: &Node) -> Option<bool> {
                node.get_property($prop_ref).and_then(Value::as_bool)
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` boolean property for node `id` from `store`.")]
            #[inline]
            pub fn $get_getter(store: &SemanticStore, id: NodeId) -> Option<bool> {
                store.get_node(id).and_then(Self::$getter_of)
            }

            #[doc = concat!("Returns `true` if `", stringify!($getter), "` is explicitly set to `true`.")]
            #[inline]
            pub fn $is_getter(&self, store: &SemanticStore) -> bool {
                self.$getter(store).unwrap_or(false)
            }

            #[doc = concat!("Returns `true` if `", stringify!($getter), "` is explicitly set to `true` on `node`.")]
            #[inline]
            pub fn $is_getter_of(node: &Node) -> bool {
                Self::$getter_of(node).unwrap_or(false)
            }

            #[doc = concat!("Returns `true` if `", stringify!($getter), "` is explicitly set to `true` for node `id` in `store`.")]
            #[inline]
            pub fn $is_getter_for(store: &SemanticStore, id: NodeId) -> bool {
                Self::$get_getter(store, id).unwrap_or(false)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on this widget in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter(&self, store: &mut SemanticStore, value: bool) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut SemanticStore, id: NodeId, value: bool) -> Result<Option<Value>, StoreError> {
                store.set_property(id, $prop_ref, Value::Bool(value))
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_set(&self, value: bool) -> Operation {
                Self::$op_set_for(self.id, value)
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_set_for(id: NodeId, value: bool) -> Operation {
                Operation::set_property(id, $prop_ref, Value::Bool(value))
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on this widget in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
                store.clear_property(id, $prop_ref)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_clear(&self) -> Operation {
                Self::$op_clear_for(self.id)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_clear_for(id: NodeId) -> Operation {
                Operation::clear_property(id, $prop_ref)
            }
        }

        impl $builder {
            #[doc = concat!("Sets the `", stringify!($getter), "` property for the new widget (§7.4).")]
            #[inline]
            #[must_use]
            pub fn $getter(mut self, value: bool) -> Self {
                self.properties.push(($prop_ref, Value::Bool(value)));
                self
            }
        }
    };
}

macro_rules! impl_enum_prop {
    ($widget:ident, $builder:ident, $enum_type:ty, $getter:ident, $getter_of:ident, $get_getter:ident, $setter:ident, $setter_for:ident, $op_set:ident, $op_set_for:ident, $clear:ident, $clear_for:ident, $op_clear:ident, $op_clear_for:ident, $prop_ref:expr, $doc:expr) => {
        impl $widget {
            #[doc = concat!("Returns the `", stringify!($getter), "` enum property if set.")]
            #[inline]
            pub fn $getter(&self, store: &SemanticStore) -> Option<$enum_type> {
                store.get_node(self.id).and_then(|n| Self::$getter_of(n))
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` enum property from a [`Node`].")]
            #[inline]
            pub fn $getter_of(node: &Node) -> Option<$enum_type> {
                node.get_property($prop_ref)
                    .and_then(Value::as_enum_token)
                    .and_then(|t| <$enum_type>::try_from(t).ok())
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` enum property for node `id` from `store`.")]
            #[inline]
            pub fn $get_getter(store: &SemanticStore, id: NodeId) -> Option<$enum_type> {
                store.get_node(id).and_then(Self::$getter_of)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on this widget in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter(&self, store: &mut SemanticStore, value: $enum_type) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut SemanticStore, id: NodeId, value: $enum_type) -> Result<Option<Value>, StoreError> {
                store.set_property(id, $prop_ref, Value::EnumToken(EnumToken::from(value)))
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_set(&self, value: $enum_type) -> Operation {
                Self::$op_set_for(self.id, value)
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_set_for(id: NodeId, value: $enum_type) -> Operation {
                Operation::set_property(id, $prop_ref, Value::EnumToken(EnumToken::from(value)))
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on this widget in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
                store.clear_property(id, $prop_ref)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_clear(&self) -> Operation {
                Self::$op_clear_for(self.id)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_clear_for(id: NodeId) -> Operation {
                Operation::clear_property(id, $prop_ref)
            }
        }

        impl $builder {
            #[doc = concat!("Sets the `", stringify!($getter), "` property for the new widget (§7.4).")]
            #[inline]
            #[must_use]
            pub fn $getter(mut self, value: $enum_type) -> Self {
                self.properties.push(($prop_ref, Value::EnumToken(EnumToken::from(value))));
                self
            }
        }
    };
}

macro_rules! impl_f64_prop {
    ($widget:ident, $builder:ident, $getter:ident, $getter_of:ident, $get_getter:ident, $setter:ident, $setter_for:ident, $op_set:ident, $op_set_for:ident, $clear:ident, $clear_for:ident, $op_clear:ident, $op_clear_for:ident, $prop_ref:expr, $doc:expr) => {
        impl $widget {
            #[doc = concat!("Returns the `", stringify!($getter), "` f64 property if set.")]
            #[inline]
            pub fn $getter(&self, store: &SemanticStore) -> Option<f64> {
                store.get_node(self.id).and_then(|n| Self::$getter_of(n))
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` f64 property from a [`Node`].")]
            #[inline]
            pub fn $getter_of(node: &Node) -> Option<f64> {
                node.get_property($prop_ref).and_then(Value::as_float64)
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` f64 property for node `id` from `store`.")]
            #[inline]
            pub fn $get_getter(store: &SemanticStore, id: NodeId) -> Option<f64> {
                store.get_node(id).and_then(Self::$getter_of)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on this widget in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter(&self, store: &mut SemanticStore, value: f64) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut SemanticStore, id: NodeId, value: f64) -> Result<Option<Value>, StoreError> {
                store.set_property(id, $prop_ref, Value::Float64(value))
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_set(&self, value: f64) -> Operation {
                Self::$op_set_for(self.id, value)
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_set_for(id: NodeId, value: f64) -> Operation {
                Operation::set_property(id, $prop_ref, Value::Float64(value))
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on this widget in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
                store.clear_property(id, $prop_ref)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_clear(&self) -> Operation {
                Self::$op_clear_for(self.id)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_clear_for(id: NodeId) -> Operation {
                Operation::clear_property(id, $prop_ref)
            }
        }

        impl $builder {
            #[doc = concat!("Sets the `", stringify!($getter), "` property for the new widget (§7.4).")]
            #[inline]
            #[must_use]
            pub fn $getter(mut self, value: f64) -> Self {
                self.properties.push(($prop_ref, Value::Float64(value)));
                self
            }
        }
    };
}

macro_rules! impl_size_prop {
    ($widget:ident, $builder:ident, $getter:ident, $getter_of:ident, $get_getter:ident, $setter:ident, $setter_for:ident, $op_set:ident, $op_set_for:ident, $clear:ident, $clear_for:ident, $op_clear:ident, $op_clear_for:ident, $prop_ref:expr, $doc:expr) => {
        impl $widget {
            #[doc = concat!("Returns the `", stringify!($getter), "` [`Size`] property if set.")]
            #[inline]
            pub fn $getter(&self, store: &SemanticStore) -> Option<Size> {
                store.get_node(self.id).and_then(|n| Self::$getter_of(n))
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` [`Size`] property from a [`Node`].")]
            #[inline]
            pub fn $getter_of(node: &Node) -> Option<Size> {
                node.get_property($prop_ref).and_then(Value::as_size)
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` [`Size`] property for node `id` from `store`.")]
            #[inline]
            pub fn $get_getter(store: &SemanticStore, id: NodeId) -> Option<Size> {
                store.get_node(id).and_then(Self::$getter_of)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on this widget in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter(&self, store: &mut SemanticStore, value: Size) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut SemanticStore, id: NodeId, value: Size) -> Result<Option<Value>, StoreError> {
                store.set_property(id, $prop_ref, Value::Size(value))
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_set(&self, value: Size) -> Operation {
                Self::$op_set_for(self.id, value)
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_set_for(id: NodeId, value: Size) -> Operation {
                Operation::set_property(id, $prop_ref, Value::Size(value))
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on this widget in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
                store.clear_property(id, $prop_ref)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_clear(&self) -> Operation {
                Self::$op_clear_for(self.id)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_clear_for(id: NodeId) -> Operation {
                Operation::clear_property(id, $prop_ref)
            }
        }

        impl $builder {
            #[doc = concat!("Sets the `", stringify!($getter), "` property for the new widget (§7.4).")]
            #[inline]
            #[must_use]
            pub fn $getter(mut self, value: Size) -> Self {
                self.properties.push(($prop_ref, Value::Size(value)));
                self
            }
        }
    };
}

macro_rules! impl_list_prop {
    ($widget:ident, $builder:ident, $getter:ident, $getter_of:ident, $get_getter:ident, $setter:ident, $setter_for:ident, $op_set:ident, $op_set_for:ident, $clear:ident, $clear_for:ident, $op_clear:ident, $op_clear_for:ident, $prop_ref:expr, $doc:expr) => {
        impl $widget {
            #[doc = concat!("Returns the `", stringify!($getter), "` list property if set.")]
            #[inline]
            #[allow(clippy::needless_lifetimes)]
            pub fn $getter<'a>(&self, store: &'a SemanticStore) -> Option<&'a [Value]> {
                store.get_node(self.id).and_then(|n| Self::$getter_of(n))
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` list property from a [`Node`].")]
            #[inline]
            pub fn $getter_of(node: &Node) -> Option<&[Value]> {
                node.get_property($prop_ref).and_then(Value::as_list)
            }

            #[doc = concat!("Returns the `", stringify!($getter), "` list property for node `id` from `store`.")]
            #[inline]
            pub fn $get_getter(store: &SemanticStore, id: NodeId) -> Option<&[Value]> {
                store.get_node(id).and_then(Self::$getter_of)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` list property on this widget in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter(&self, store: &mut SemanticStore, value: impl IntoIterator<Item = impl Into<Value>>) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` list property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut SemanticStore, id: NodeId, value: impl IntoIterator<Item = impl Into<Value>>) -> Result<Option<Value>, StoreError> {
                store.set_property(id, $prop_ref, Value::List(value.into_iter().map(Into::into).collect()))
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_set(&self, value: impl IntoIterator<Item = impl Into<Value>>) -> Operation {
                Self::$op_set_for(self.id, value)
            }

            #[doc = concat!("Returns a [`Operation::SetProperty`] operation setting `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_set_for(id: NodeId, value: impl IntoIterator<Item = impl Into<Value>>) -> Operation {
                Operation::set_property(id, $prop_ref, Value::List(value.into_iter().map(Into::into).collect()))
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on this widget in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
                store.clear_property(id, $prop_ref)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` on this widget (§13).")]
            #[inline]
            pub fn $op_clear(&self) -> Operation {
                Self::$op_clear_for(self.id)
            }

            #[doc = concat!("Returns a [`Operation::ClearProperty`] operation clearing `", stringify!($getter), "` (§13).")]
            #[inline]
            pub fn $op_clear_for(id: NodeId) -> Operation {
                Operation::clear_property(id, $prop_ref)
            }
        }

        impl $builder {
            #[doc = concat!("Sets the `", stringify!($getter), "` property for the new widget (§7.4).")]
            #[inline]
            #[must_use]
            pub fn $getter(mut self, value: impl IntoIterator<Item = impl Into<Value>>) -> Self {
                self.properties.push(($prop_ref, Value::List(value.into_iter().map(Into::into).collect())));
                self
            }
        }
    };
}

macro_rules! impl_model_ref_prop {
    ($widget:ident, $builder:ident) => {
        impl $widget {
            #[doc = "Returns the referenced [`ModelId`] if set (§8)."]
            #[inline]
            pub fn model_ref(&self, store: &SemanticStore) -> Option<ModelId> {
                store.get_node(self.id).and_then(|n| Self::model_ref_of(n))
            }

            #[doc = "Returns the referenced [`ModelId`] from a [`Node`] (§8)."]
            #[inline]
            pub fn model_ref_of(node: &Node) -> Option<ModelId> {
                node.model_ref()
            }

            #[doc = "Returns the referenced [`ModelId`] for node `id` from `store` (§8)."]
            #[inline]
            pub fn get_model_ref(store: &SemanticStore, id: NodeId) -> Option<ModelId> {
                store.get_node(id).and_then(Self::model_ref_of)
            }

            #[doc = "Sets the `model_ref` property on this widget in `store` (§13 SET_PROPERTY, §8)."]
            #[inline]
            pub fn set_model_ref(&self, store: &mut SemanticStore, model_id: impl Into<ModelId>) -> Result<Option<Value>, StoreError> {
                Self::set_model_ref_for(store, self.id, model_id)
            }

            #[doc = "Sets the `model_ref` property on the given node in `store` (§13 SET_PROPERTY, §8)."]
            #[inline]
            pub fn set_model_ref_for(store: &mut SemanticStore, id: NodeId, model_id: impl Into<ModelId>) -> Result<Option<Value>, StoreError> {
                let mid = model_id.into();
                store.set_property(id, PropertyRef::MODEL_REF, Value::UnsignedInt(mid.get()))
            }

            #[doc = "Returns a [`Operation::SetProperty`] operation setting `model_ref` on this widget (§13)."]
            #[inline]
            pub fn op_set_model_ref(&self, model_id: impl Into<ModelId>) -> Operation {
                Self::op_set_model_ref_for(self.id, model_id)
            }

            #[doc = "Returns a [`Operation::SetProperty`] operation setting `model_ref` (§13)."]
            #[inline]
            pub fn op_set_model_ref_for(id: NodeId, model_id: impl Into<ModelId>) -> Operation {
                let mid = model_id.into();
                Operation::set_property(id, PropertyRef::MODEL_REF, Value::UnsignedInt(mid.get()))
            }

            #[doc = "Clears the `model_ref` property on this widget in `store` (§13 CLEAR_PROPERTY)."]
            #[inline]
            pub fn clear_model_ref(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
                Self::clear_model_ref_for(store, self.id)
            }

            #[doc = "Clears the `model_ref` property on the given node in `store` (§13 CLEAR_PROPERTY)."]
            #[inline]
            pub fn clear_model_ref_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
                store.clear_property(id, PropertyRef::MODEL_REF)
            }

            #[doc = "Returns a [`Operation::ClearProperty`] operation clearing `model_ref` on this widget (§13)."]
            #[inline]
            pub fn op_clear_model_ref(&self) -> Operation {
                Self::op_clear_model_ref_for(self.id)
            }

            #[doc = "Returns a [`Operation::ClearProperty`] operation clearing `model_ref` (§13)."]
            #[inline]
            pub fn op_clear_model_ref_for(id: NodeId) -> Operation {
                Operation::clear_property(id, PropertyRef::MODEL_REF)
            }
        }

        impl $builder {
            #[doc = "Sets the referenced `model_ref` for the new collection widget (§8)."]
            #[inline]
            #[must_use]
            pub fn model_ref(mut self, model_id: impl Into<ModelId>) -> Self {
                let mid = model_id.into();
                self.properties.push((PropertyRef::MODEL_REF, Value::UnsignedInt(mid.get())));
                self
            }
        }
    };
}

macro_rules! impl_common_layout_props {
    ($widget:ident, $builder:ident) => {
        impl_enum_prop!($widget, $builder, HorizontalAlignment, horizontal_alignment, horizontal_alignment_of, get_horizontal_alignment, set_horizontal_alignment, set_horizontal_alignment_for, op_set_horizontal_alignment, op_set_horizontal_alignment_for, clear_horizontal_alignment, clear_horizontal_alignment_for, op_clear_horizontal_alignment, op_clear_horizontal_alignment_for, PropertyRef::HORIZONTAL_ALIGNMENT, "Horizontal alignment within parent layout (§7.4).");
        impl_enum_prop!($widget, $builder, VerticalAlignment, vertical_alignment, vertical_alignment_of, get_vertical_alignment, set_vertical_alignment, set_vertical_alignment_for, op_set_vertical_alignment, op_set_vertical_alignment_for, clear_vertical_alignment, clear_vertical_alignment_for, op_clear_vertical_alignment, op_clear_vertical_alignment_for, PropertyRef::VERTICAL_ALIGNMENT, "Vertical alignment within parent layout (§7.4).");
        impl_f64_prop!($widget, $builder, grow, grow_of, get_grow, set_grow, set_grow_for, op_set_grow, op_set_grow_for, clear_grow, clear_grow_for, op_clear_grow, op_clear_grow_for, PropertyRef::GROW, "Relative flex grow weight (§7.4).");
        impl_f64_prop!($widget, $builder, shrink, shrink_of, get_shrink, set_shrink, set_shrink_for, op_set_shrink, op_set_shrink_for, clear_shrink, clear_shrink_for, op_clear_shrink, op_clear_shrink_for, PropertyRef::SHRINK, "Relative flex shrink weight (§7.4).");
        impl_size_prop!($widget, $builder, preferred_size, preferred_size_of, get_preferred_size, set_preferred_size, set_preferred_size_for, op_set_preferred_size, op_set_preferred_size_for, clear_preferred_size, clear_preferred_size_for, op_clear_preferred_size, op_clear_preferred_size_for, PropertyRef::PREFERRED_SIZE, "Preferred logical dimensions (§7.4).");
        impl_size_prop!($widget, $builder, minimum_size, minimum_size_of, get_minimum_size, set_minimum_size, set_minimum_size_for, op_set_minimum_size, op_set_minimum_size_for, clear_minimum_size, clear_minimum_size_for, op_clear_minimum_size, op_clear_minimum_size_for, PropertyRef::MINIMUM_SIZE, "Minimum logical dimensions (§7.4).");
        impl_size_prop!($widget, $builder, maximum_size, maximum_size_of, get_maximum_size, set_maximum_size, set_maximum_size_for, op_set_maximum_size, op_set_maximum_size_for, clear_maximum_size, clear_maximum_size_for, op_clear_maximum_size, op_clear_maximum_size_for, PropertyRef::MAXIMUM_SIZE, "Maximum logical dimensions (§7.4).");
    };
}

macro_rules! impl_common_state_props {
    ($widget:ident, $builder:ident) => {
        impl_enum_prop!($widget, $builder, Visibility, visibility, visibility_of, get_visibility, set_visibility, set_visibility_for, op_set_visibility, op_set_visibility_for, clear_visibility, clear_visibility_for, op_clear_visibility, op_clear_visibility_for, PropertyRef::VISIBILITY, "Visibility and layout participation (§7.4).");
        impl_bool_prop!($widget, $builder, enabled, enabled_of, get_enabled, is_enabled, is_enabled_of, is_enabled_for, set_enabled, set_enabled_for, op_set_enabled, op_set_enabled_for, clear_enabled, clear_enabled_for, op_clear_enabled, op_clear_enabled_for, PropertyRef::ENABLED, "Whether the node is interactive (§7.4).");
        impl_bool_prop!($widget, $builder, busy, busy_of, get_busy, is_busy, is_busy_of, is_busy_for, set_busy, set_busy_for, op_set_busy, op_set_busy_for, clear_busy, clear_busy_for, op_clear_busy, op_clear_busy_for, PropertyRef::BUSY, "Whether the node is performing an asynchronous operation (§7.4).");
    };
}

macro_rules! impl_container_spacing_props {
    ($widget:ident, $builder:ident) => {
        impl_enum_prop!($widget, $builder, SpacingRole, spacing_role, spacing_role_of, get_spacing_role, set_spacing_role, set_spacing_role_for, op_set_spacing_role, op_set_spacing_role_for, clear_spacing_role, clear_spacing_role_for, op_clear_spacing_role, op_clear_spacing_role_for, PropertyRef::SPACING_ROLE, "Semantic inter-item spacing (§7.4).");
        impl_enum_prop!($widget, $builder, PaddingRole, padding_role, padding_role_of, get_padding_role, set_padding_role, set_padding_role_for, op_set_padding_role, op_set_padding_role_for, clear_padding_role, clear_padding_role_for, op_clear_padding_role, op_clear_padding_role_for, PropertyRef::PADDING_ROLE, "Semantic container padding (§7.4).");
    };
}

// =============================================================================
// 1. Surface (§7.2, §7.3 Required Tier, TypeRef::SURFACE / id 1)
// =============================================================================

impl_widget_boilerplate!(
    Surface,
    SurfaceBuilder,
    TypeRef::SURFACE,
    "Top-level window or surface content root (§7.2, §7.3 Required Tier)."
);
impl_string_prop!(Surface, SurfaceBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Window title or surface accessibility label.");
impl_string_prop!(Surface, SurfaceBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description.");
impl_common_state_props!(Surface, SurfaceBuilder);
impl_container_spacing_props!(Surface, SurfaceBuilder);
impl_common_layout_props!(Surface, SurfaceBuilder);

// =============================================================================
// 2. Row (§7.2, §7.3 Required Tier, TypeRef::ROW / id 3)
// =============================================================================

impl_widget_boilerplate!(
    Row,
    RowBuilder,
    TypeRef::ROW,
    "Ordered horizontal layout container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Row, RowBuilder);
impl_common_state_props!(Row, RowBuilder);
impl_common_layout_props!(Row, RowBuilder);
impl_string_prop!(Row, RowBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description.");

// =============================================================================
// 3. Column (§7.2, §7.3 Required Tier, TypeRef::COLUMN / id 4)
// =============================================================================

impl_widget_boilerplate!(
    Column,
    ColumnBuilder,
    TypeRef::COLUMN,
    "Ordered vertical layout container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Column, ColumnBuilder);
impl_common_state_props!(Column, ColumnBuilder);
impl_common_layout_props!(Column, ColumnBuilder);
impl_string_prop!(Column, ColumnBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description.");

// =============================================================================
// 4. Grid (§7.2, §7.3 Required Tier, TypeRef::GRID / id 5)
// =============================================================================

impl_widget_boilerplate!(
    Grid,
    GridBuilder,
    TypeRef::GRID,
    "Two-dimensional row/column layout container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Grid, GridBuilder);
impl_common_state_props!(Grid, GridBuilder);
impl_common_layout_props!(Grid, GridBuilder);
impl_list_prop!(Grid, GridBuilder, columns, columns_of, get_columns, set_columns, set_columns_for, op_set_columns, op_set_columns_for, clear_columns, clear_columns_for, op_clear_columns, op_clear_columns_for, PropertyRef::COLUMNS, "Grid column specifications or metadata (§7.4).");
impl_string_prop!(Grid, GridBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description.");

// =============================================================================
// 5. Spacer (§7.2, §7.3 Required Tier, TypeRef::SPACER / id 6)
// =============================================================================

impl_widget_boilerplate!(
    Spacer,
    SpacerBuilder,
    TypeRef::SPACER,
    "Flexible empty layout item for spacing and alignment (§7.2, §7.3 Required Tier)."
);
impl_common_layout_props!(Spacer, SpacerBuilder);
impl_enum_prop!(Spacer, SpacerBuilder, Visibility, visibility, visibility_of, get_visibility, set_visibility, set_visibility_for, op_set_visibility, op_set_visibility_for, clear_visibility, clear_visibility_for, op_clear_visibility, op_clear_visibility_for, PropertyRef::VISIBILITY, "Visibility and layout participation (§7.4).");

// =============================================================================
// 6. Separator (§7.2, §7.3 Required Tier, TypeRef::SEPARATOR / id 7)
// =============================================================================

impl_widget_boilerplate!(
    Separator,
    SeparatorBuilder,
    TypeRef::SEPARATOR,
    "Semantic visual grouping line or divider (§7.2, §7.3 Required Tier)."
);
impl_common_layout_props!(Separator, SeparatorBuilder);
impl_common_state_props!(Separator, SeparatorBuilder);
impl_string_prop!(Separator, SeparatorBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description.");

// =============================================================================
// 7. Text (§7.2, §7.3 Required Tier, TypeRef::TEXT / id 9)
// =============================================================================

impl_widget_boilerplate!(
    Text,
    TextBuilder,
    TypeRef::TEXT,
    "Non-editable static or dynamic text label (§7.2, §7.3 Required Tier)."
);
impl_string_prop!(Text, TextBuilder, text, text_of, get_text, set_text, set_text_for, op_set_text, op_set_text_for, clear_text, clear_text_for, op_clear_text, op_clear_text_for, PropertyRef::TEXT, "Primary text content (§7.4).");
impl_enum_prop!(Text, TextBuilder, TextRole, role, role_of, get_role, set_role, set_role_for, op_set_role, op_set_role_for, clear_role, clear_role_for, op_clear_role, op_clear_role_for, PropertyRef::ROLE, "Semantic text role (§7.5).");
impl_enum_prop!(Text, TextBuilder, Importance, importance, importance_of, get_importance, set_importance, set_importance_for, op_set_importance, op_set_importance_for, clear_importance, clear_importance_for, op_clear_importance, op_clear_importance_for, PropertyRef::ROLE, "Semantic emphasis level (§7.5).");
impl_string_prop!(Text, TextBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Primary label or accessibility name (§7.4).");
impl_string_prop!(Text, TextBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_string_prop!(Text, TextBuilder, value_description, value_description_of, get_value_description, set_value_description, set_value_description_for, op_set_value_description, op_set_value_description_for, clear_value_description, clear_value_description_for, op_clear_value_description, op_clear_value_description_for, PropertyRef::VALUE_DESCRIPTION, "Human-readable value description (§7.4).");
impl_common_state_props!(Text, TextBuilder);
impl_common_layout_props!(Text, TextBuilder);

// =============================================================================
// 8. RichText (§7.2, §7.3 Required Tier, TypeRef::RICHTEXT / id 10)
// =============================================================================

impl_widget_boilerplate!(
    RichText,
    RichTextBuilder,
    TypeRef::RICHTEXT,
    "Selectable structured text with semantic annotations (§7.2, §7.3 Required Tier, §9)."
);
impl_string_prop!(RichText, RichTextBuilder, text, text_of, get_text, set_text, set_text_for, op_set_text, op_set_text_for, clear_text, clear_text_for, op_clear_text, op_clear_text_for, PropertyRef::TEXT, "Primary structured text content (§7.4, §9).");
impl_enum_prop!(RichText, RichTextBuilder, TextRole, role, role_of, get_role, set_role, set_role_for, op_set_role, op_set_role_for, clear_role, clear_role_for, op_clear_role, op_clear_role_for, PropertyRef::ROLE, "Semantic text role (§7.5).");
impl_enum_prop!(RichText, RichTextBuilder, Importance, importance, importance_of, get_importance, set_importance, set_importance_for, op_set_importance, op_set_importance_for, clear_importance, clear_importance_for, op_clear_importance, op_clear_importance_for, PropertyRef::ROLE, "Semantic emphasis level (§7.5).");
impl_string_prop!(RichText, RichTextBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Accessibility label (§7.4).");
impl_string_prop!(RichText, RichTextBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_bool_prop!(RichText, RichTextBuilder, read_only, read_only_of, get_read_only, is_read_only, is_read_only_of, is_read_only_for, set_read_only, set_read_only_for, op_set_read_only, op_set_read_only_for, clear_read_only, clear_read_only_for, op_clear_read_only, op_clear_read_only_for, PropertyRef::READ_ONLY, "Whether the text is read-only (§7.4).");
impl_common_state_props!(RichText, RichTextBuilder);
impl_common_layout_props!(RichText, RichTextBuilder);

// =============================================================================
// 9. Button (§7.2, §7.3 Required Tier, TypeRef::BUTTON / id 11)
// =============================================================================

impl_widget_boilerplate!(
    Button,
    ButtonBuilder,
    TypeRef::BUTTON,
    "Momentary action trigger (§7.2, §7.3 Required Tier)."
);
impl_string_prop!(Button, ButtonBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Button label text (§7.4).");
impl_enum_prop!(Button, ButtonBuilder, ActionRole, role, role_of, get_role, set_role, set_role_for, op_set_role, op_set_role_for, clear_role, clear_role_for, op_clear_role, op_clear_role_for, PropertyRef::ROLE, "Semantic action role (§7.5).");
impl_enum_prop!(Button, ButtonBuilder, Importance, importance, importance_of, get_importance, set_importance, set_importance_for, op_set_importance, op_set_importance_for, clear_importance, clear_importance_for, op_clear_importance, op_clear_importance_for, PropertyRef::ROLE, "Semantic emphasis level (§7.5).");
impl_string_prop!(Button, ButtonBuilder, action_key, action_key_of, get_action_key, set_action_key, set_action_key_for, op_set_action_key, op_set_action_key_for, clear_action_key, clear_action_key_for, op_clear_action_key, op_clear_action_key_for, PropertyRef::ACTION_KEY, "Opaque semantic action key (§7.7).");
impl_list_prop!(Button, ButtonBuilder, actions, actions_of, get_actions, set_actions, set_actions_for, op_set_actions, op_set_actions_for, clear_actions, clear_actions_for, op_clear_actions, op_clear_actions_for, PropertyRef::ACTIONS, "List of supported semantic actions (§7.4).");
impl_string_prop!(Button, ButtonBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_bool_prop!(Button, ButtonBuilder, selected, selected_of, get_selected, is_selected, is_selected_of, is_selected_for, set_selected, set_selected_for, op_set_selected, op_set_selected_for, clear_selected, clear_selected_for, op_clear_selected, op_clear_selected_for, PropertyRef::SELECTED, "Selected state (§7.4).");
impl_common_state_props!(Button, ButtonBuilder);
impl_common_layout_props!(Button, ButtonBuilder);

// =============================================================================
// 10. Toggle (§7.2, §7.3 Required Tier, TypeRef::TOGGLE / id 12)
// =============================================================================

impl_widget_boilerplate!(
    Toggle,
    ToggleBuilder,
    TypeRef::TOGGLE,
    "User-editable boolean state control (§7.2, §7.3 Required Tier).\n\n\
     Per §7.2, checkbox and switch share the exact same semantic state machine (`TypeRef::TOGGLE`)\n\
     with `value: bool` and an advisory `presentation_hint` enum."
);

impl Toggle {
    /// Convenience constructor creating a [`ToggleBuilder`] with [`TogglePresentationHint::Checkbox`].
    #[inline]
    pub fn checkbox(id: impl Into<NodeId>) -> ToggleBuilder {
        ToggleBuilder::new(id).presentation_hint(TogglePresentationHint::Checkbox)
    }

    /// Convenience constructor creating a [`ToggleBuilder`] with [`TogglePresentationHint::Switch`].
    #[inline]
    pub fn switch(id: impl Into<NodeId>) -> ToggleBuilder {
        ToggleBuilder::new(id).presentation_hint(TogglePresentationHint::Switch)
    }

    /// Convenience constructor creating a [`ToggleBuilder`] with [`TogglePresentationHint::Automatic`].
    #[inline]
    pub fn automatic(id: impl Into<NodeId>) -> ToggleBuilder {
        ToggleBuilder::new(id).presentation_hint(TogglePresentationHint::Automatic)
    }
}

impl_bool_prop!(Toggle, ToggleBuilder, value, value_of, get_value, is_value_set, is_value_set_of, is_value_set_for, set_value, set_value_for, op_set_value, op_set_value_for, clear_value, clear_value_for, op_clear_value, op_clear_value_for, PropertyRef::VALUE, "Boolean toggle value (§7.2, §7.4).");
impl_enum_prop!(Toggle, ToggleBuilder, TogglePresentationHint, presentation_hint, presentation_hint_of, get_presentation_hint, set_presentation_hint, set_presentation_hint_for, op_set_presentation_hint, op_set_presentation_hint_for, clear_presentation_hint, clear_presentation_hint_for, op_clear_presentation_hint, op_clear_presentation_hint_for, PropertyRef::PRESENTATION_HINT, "Advisory presentation hint (checkbox | switch | automatic) (§7.2).");
impl_string_prop!(Toggle, ToggleBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Toggle label text (§7.4).");
impl_enum_prop!(Toggle, ToggleBuilder, Importance, importance, importance_of, get_importance, set_importance, set_importance_for, op_set_importance, op_set_importance_for, clear_importance, clear_importance_for, op_clear_importance, op_clear_importance_for, PropertyRef::ROLE, "Semantic emphasis level (§7.5).");
impl_string_prop!(Toggle, ToggleBuilder, action_key, action_key_of, get_action_key, set_action_key, set_action_key_for, op_set_action_key, op_set_action_key_for, clear_action_key, clear_action_key_for, op_clear_action_key, op_clear_action_key_for, PropertyRef::ACTION_KEY, "Opaque action key (§7.7).");
impl_string_prop!(Toggle, ToggleBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_string_prop!(Toggle, ToggleBuilder, value_description, value_description_of, get_value_description, set_value_description, set_value_description_for, op_set_value_description, op_set_value_description_for, clear_value_description, clear_value_description_for, op_clear_value_description, op_clear_value_description_for, PropertyRef::VALUE_DESCRIPTION, "Human-readable value description (§7.4).");
impl_enum_prop!(Toggle, ToggleBuilder, ValidationState, validation_state, validation_state_of, get_validation_state, set_validation_state, set_validation_state_for, op_set_validation_state, op_set_validation_state_for, clear_validation_state, clear_validation_state_for, op_clear_validation_state, op_clear_validation_state_for, PropertyRef::VALIDATION_STATE, "Validation state (§7.4).");
impl_bool_prop!(Toggle, ToggleBuilder, read_only, read_only_of, get_read_only, is_read_only, is_read_only_of, is_read_only_for, set_read_only, set_read_only_for, op_set_read_only, op_set_read_only_for, clear_read_only, clear_read_only_for, op_clear_read_only, op_clear_read_only_for, PropertyRef::READ_ONLY, "Whether the toggle is read-only (§7.4).");
impl_common_state_props!(Toggle, ToggleBuilder);
impl_common_layout_props!(Toggle, ToggleBuilder);

// =============================================================================
// 11. TextInput (§7.2, §7.3 Required Tier, TypeRef::TEXT_INPUT / id 13)
// =============================================================================

impl_widget_boilerplate!(
    TextInput,
    TextInputBuilder,
    TypeRef::TEXT_INPUT,
    "Single-line text editing field (§7.2, §7.3 Required Tier, §22.6)."
);
impl_string_prop!(TextInput, TextInputBuilder, value, value_of, get_value, set_value, set_value_for, op_set_value, op_set_value_for, clear_value, clear_value_for, op_clear_value, op_clear_value_for, PropertyRef::VALUE, "Current text input value (§7.4).");
impl_string_prop!(TextInput, TextInputBuilder, text, text_of, get_text, set_text, set_text_for, op_set_text, op_set_text_for, clear_text, clear_text_for, op_clear_text, op_clear_text_for, PropertyRef::TEXT, "Text content alias (§7.4).");
impl_string_prop!(TextInput, TextInputBuilder, placeholder, placeholder_of, get_placeholder, set_placeholder, set_placeholder_for, op_set_placeholder, op_set_placeholder_for, clear_placeholder, clear_placeholder_for, op_clear_placeholder, op_clear_placeholder_for, PropertyRef::PLACEHOLDER, "Placeholder text (§7.4).");
impl_enum_prop!(TextInput, TextInputBuilder, InputRole, role, role_of, get_role, set_role, set_role_for, op_set_role, op_set_role_for, clear_role, clear_role_for, op_clear_role, op_clear_role_for, PropertyRef::ROLE, "Semantic input role (plain | search | secure | command) (§7.5).");
impl_enum_prop!(TextInput, TextInputBuilder, Importance, importance, importance_of, get_importance, set_importance, set_importance_for, op_set_importance, op_set_importance_for, clear_importance, clear_importance_for, op_clear_importance, op_clear_importance_for, PropertyRef::ROLE, "Semantic emphasis level (§7.5).");
impl_string_prop!(TextInput, TextInputBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Input field label (§7.4).");
impl_string_prop!(TextInput, TextInputBuilder, action_key, action_key_of, get_action_key, set_action_key, set_action_key_for, op_set_action_key, op_set_action_key_for, clear_action_key, clear_action_key_for, op_clear_action_key, op_clear_action_key_for, PropertyRef::ACTION_KEY, "Opaque action key (§7.7).");
impl_string_prop!(TextInput, TextInputBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_string_prop!(TextInput, TextInputBuilder, value_description, value_description_of, get_value_description, set_value_description, set_value_description_for, op_set_value_description, op_set_value_description_for, clear_value_description, clear_value_description_for, op_clear_value_description, op_clear_value_description_for, PropertyRef::VALUE_DESCRIPTION, "Human-readable value description (§7.4).");
impl_enum_prop!(TextInput, TextInputBuilder, ValidationState, validation_state, validation_state_of, get_validation_state, set_validation_state, set_validation_state_for, op_set_validation_state, op_set_validation_state_for, clear_validation_state, clear_validation_state_for, op_clear_validation_state, op_clear_validation_state_for, PropertyRef::VALIDATION_STATE, "Validation state (§7.4).");
impl_bool_prop!(TextInput, TextInputBuilder, read_only, read_only_of, get_read_only, is_read_only, is_read_only_of, is_read_only_for, set_read_only, set_read_only_for, op_set_read_only, op_set_read_only_for, clear_read_only, clear_read_only_for, op_clear_read_only, op_clear_read_only_for, PropertyRef::READ_ONLY, "Whether the input is read-only (§7.4).");
impl_common_state_props!(TextInput, TextInputBuilder);
impl_common_layout_props!(TextInput, TextInputBuilder);

// =============================================================================
// 12. TextArea (§7.2, §7.3 Required Tier, TypeRef::TEXT_AREA / id 14)
// =============================================================================

impl_widget_boilerplate!(
    TextArea,
    TextAreaBuilder,
    TypeRef::TEXT_AREA,
    "Multi-line text editing view (§7.2, §7.3 Required Tier, §22.6)."
);
impl_string_prop!(TextArea, TextAreaBuilder, value, value_of, get_value, set_value, set_value_for, op_set_value, op_set_value_for, clear_value, clear_value_for, op_clear_value, op_clear_value_for, PropertyRef::VALUE, "Current text area value (§7.4).");
impl_string_prop!(TextArea, TextAreaBuilder, text, text_of, get_text, set_text, set_text_for, op_set_text, op_set_text_for, clear_text, clear_text_for, op_clear_text, op_clear_text_for, PropertyRef::TEXT, "Text content alias (§7.4).");
impl_string_prop!(TextArea, TextAreaBuilder, placeholder, placeholder_of, get_placeholder, set_placeholder, set_placeholder_for, op_set_placeholder, op_set_placeholder_for, clear_placeholder, clear_placeholder_for, op_clear_placeholder, op_clear_placeholder_for, PropertyRef::PLACEHOLDER, "Placeholder text (§7.4).");
impl_enum_prop!(TextArea, TextAreaBuilder, InputRole, role, role_of, get_role, set_role, set_role_for, op_set_role, op_set_role_for, clear_role, clear_role_for, op_clear_role, op_clear_role_for, PropertyRef::ROLE, "Semantic input role (§7.5).");
impl_enum_prop!(TextArea, TextAreaBuilder, Importance, importance, importance_of, get_importance, set_importance, set_importance_for, op_set_importance, op_set_importance_for, clear_importance, clear_importance_for, op_clear_importance, op_clear_importance_for, PropertyRef::ROLE, "Semantic emphasis level (§7.5).");
impl_string_prop!(TextArea, TextAreaBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Text area label (§7.4).");
impl_string_prop!(TextArea, TextAreaBuilder, action_key, action_key_of, get_action_key, set_action_key, set_action_key_for, op_set_action_key, op_set_action_key_for, clear_action_key, clear_action_key_for, op_clear_action_key, op_clear_action_key_for, PropertyRef::ACTION_KEY, "Opaque action key (§7.7).");
impl_string_prop!(TextArea, TextAreaBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_string_prop!(TextArea, TextAreaBuilder, value_description, value_description_of, get_value_description, set_value_description, set_value_description_for, op_set_value_description, op_set_value_description_for, clear_value_description, clear_value_description_for, op_clear_value_description, op_clear_value_description_for, PropertyRef::VALUE_DESCRIPTION, "Human-readable value description (§7.4).");
impl_enum_prop!(TextArea, TextAreaBuilder, ValidationState, validation_state, validation_state_of, get_validation_state, set_validation_state, set_validation_state_for, op_set_validation_state, op_set_validation_state_for, clear_validation_state, clear_validation_state_for, op_clear_validation_state, op_clear_validation_state_for, PropertyRef::VALIDATION_STATE, "Validation state (§7.4).");
impl_bool_prop!(TextArea, TextAreaBuilder, read_only, read_only_of, get_read_only, is_read_only, is_read_only_of, is_read_only_for, set_read_only, set_read_only_for, op_set_read_only, op_set_read_only_for, clear_read_only, clear_read_only_for, op_clear_read_only, op_clear_read_only_for, PropertyRef::READ_ONLY, "Whether the text area is read-only (§7.4).");
impl_common_state_props!(TextArea, TextAreaBuilder);
impl_common_layout_props!(TextArea, TextAreaBuilder);

// =============================================================================
// 13. Progress (§7.2, §7.3 Required Tier, TypeRef::PROGRESS / id 15)
// =============================================================================

impl_widget_boilerplate!(
    Progress,
    ProgressBuilder,
    TypeRef::PROGRESS,
    "Determinate or indeterminate progress indicator (§7.2, §7.3 Required Tier)."
);
impl_f64_prop!(Progress, ProgressBuilder, value, value_of, get_value, set_value, set_value_for, op_set_value, op_set_value_for, clear_value, clear_value_for, op_clear_value, op_clear_value_for, PropertyRef::VALUE, "Determinate progress value in [0.0, 1.0] (§7.4).");
impl_string_prop!(Progress, ProgressBuilder, value_description, value_description_of, get_value_description, set_value_description, set_value_description_for, op_set_value_description, op_set_value_description_for, clear_value_description, clear_value_description_for, op_clear_value_description, op_clear_value_description_for, PropertyRef::VALUE_DESCRIPTION, "Human-readable progress description (e.g. '62%') (§7.4).");
impl_string_prop!(Progress, ProgressBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Progress label text (§7.4).");
impl_string_prop!(Progress, ProgressBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_common_state_props!(Progress, ProgressBuilder);
impl_common_layout_props!(Progress, ProgressBuilder);

// =============================================================================
// 14. Image (§7.2, §7.3 Required Tier, TypeRef::IMAGE / id 16)
// =============================================================================

impl_widget_boilerplate!(
    Image,
    ImageBuilder,
    TypeRef::IMAGE,
    "Raster or vector image resource display (§7.2, §7.3 Required Tier, §14)."
);

impl Image {
    #[doc = "Returns the content-addressed [`ResourceHash`] if set (§14)."]
    #[inline]
    pub fn resource(&self, store: &SemanticStore) -> Option<ResourceHash> {
        store.get_node(self.id).and_then(Self::resource_of)
    }

    #[doc = "Returns the content-addressed [`ResourceHash`] from a [`Node`] (§14)."]
    #[inline]
    pub fn resource_of(node: &Node) -> Option<ResourceHash> {
        node.get_property(PropertyRef::RESOURCE).and_then(Value::as_resource_hash)
    }

    #[doc = "Returns the content-addressed [`ResourceHash`] for node `id` from `store` (§14)."]
    #[inline]
    pub fn get_resource(store: &SemanticStore, id: NodeId) -> Option<ResourceHash> {
        store.get_node(id).and_then(Self::resource_of)
    }

    #[doc = "Sets the `resource` property on this widget in `store` (§13 SET_PROPERTY, §14)."]
    #[inline]
    pub fn set_resource(&self, store: &mut SemanticStore, hash: impl Into<ResourceHash>) -> Result<Option<Value>, StoreError> {
        Self::set_resource_for(store, self.id, hash)
    }

    #[doc = "Sets the `resource` property on the given node in `store` (§13 SET_PROPERTY, §14)."]
    #[inline]
    pub fn set_resource_for(store: &mut SemanticStore, id: NodeId, hash: impl Into<ResourceHash>) -> Result<Option<Value>, StoreError> {
        store.set_property(id, PropertyRef::RESOURCE, Value::ResourceHash(hash.into()))
    }

    #[doc = "Returns a [`Operation::SetProperty`] operation setting `resource` on this widget (§13, §14)."]
    #[inline]
    pub fn op_set_resource(&self, hash: impl Into<ResourceHash>) -> Operation {
        Self::op_set_resource_for(self.id, hash)
    }

    #[doc = "Returns a [`Operation::SetProperty`] operation setting `resource` (§13, §14)."]
    #[inline]
    pub fn op_set_resource_for(id: NodeId, hash: impl Into<ResourceHash>) -> Operation {
        Operation::set_property(id, PropertyRef::RESOURCE, Value::ResourceHash(hash.into()))
    }

    #[doc = "Clears the `resource` property on this widget in `store` (§13 CLEAR_PROPERTY)."]
    #[inline]
    pub fn clear_resource(&self, store: &mut SemanticStore) -> Result<Option<Value>, StoreError> {
        Self::clear_resource_for(store, self.id)
    }

    #[doc = "Clears the `resource` property on the given node in `store` (§13 CLEAR_PROPERTY)."]
    #[inline]
    pub fn clear_resource_for(store: &mut SemanticStore, id: NodeId) -> Result<Option<Value>, StoreError> {
        store.clear_property(id, PropertyRef::RESOURCE)
    }

    #[doc = "Returns a [`Operation::ClearProperty`] operation clearing `resource` on this widget (§13)."]
    #[inline]
    pub fn op_clear_resource(&self) -> Operation {
        Self::op_clear_resource_for(self.id)
    }

    #[doc = "Returns a [`Operation::ClearProperty`] operation clearing `resource` (§13)."]
    #[inline]
    pub fn op_clear_resource_for(id: NodeId) -> Operation {
        Operation::clear_property(id, PropertyRef::RESOURCE)
    }
}

impl ImageBuilder {
    #[doc = "Sets the image [`ResourceHash`] for the new widget (§14)."]
    #[inline]
    #[must_use]
    pub fn resource(mut self, hash: impl Into<ResourceHash>) -> Self {
        self.properties.push((PropertyRef::RESOURCE, Value::ResourceHash(hash.into())));
        self
    }
}

impl_string_prop!(Image, ImageBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Image alt text or accessibility label (§7.4).");
impl_string_prop!(Image, ImageBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_common_state_props!(Image, ImageBuilder);
impl_common_layout_props!(Image, ImageBuilder);

// =============================================================================
// 15. Scroll (§7.2, §7.3 Required Tier, TypeRef::SCROLL / id 8)
// =============================================================================

impl_widget_boilerplate!(
    Scroll,
    ScrollBuilder,
    TypeRef::SCROLL,
    "Scrollable viewport container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Scroll, ScrollBuilder);
impl_common_state_props!(Scroll, ScrollBuilder);
impl_common_layout_props!(Scroll, ScrollBuilder);
impl_string_prop!(Scroll, ScrollBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");

// =============================================================================
// 16. List (§7.2, §7.3 Required Tier, TypeRef::LIST / id 17)
// =============================================================================

impl_widget_boilerplate!(
    List,
    ListBuilder,
    TypeRef::LIST,
    "Virtualized one-dimensional collection of items (§7.2, §7.3 Required Tier, §8)."
);
impl_model_ref_prop!(List, ListBuilder);
impl_list_prop!(List, ListBuilder, items, items_of, get_items, set_items, set_items_for, op_set_items, op_set_items_for, clear_items, clear_items_for, op_clear_items, op_clear_items_for, PropertyRef::ITEMS, "Inline item list for small un-virtualized collections (§7.4).");
impl_enum_prop!(List, ListBuilder, SelectionMode, selection_mode, selection_mode_of, get_selection_mode, set_selection_mode, set_selection_mode_for, op_set_selection_mode, op_set_selection_mode_for, clear_selection_mode, clear_selection_mode_for, op_clear_selection_mode, op_clear_selection_mode_for, PropertyRef::SELECTION_MODE, "Selection mode (none | single | multiple) (§8).");
impl_bool_prop!(List, ListBuilder, selected, selected_of, get_selected, is_selected, is_selected_of, is_selected_for, set_selected, set_selected_for, op_set_selected, op_set_selected_for, clear_selected, clear_selected_for, op_clear_selected, op_clear_selected_for, PropertyRef::SELECTED, "Selection state (§7.4).");
impl_string_prop!(List, ListBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Collection label or title (§7.4).");
impl_string_prop!(List, ListBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_common_state_props!(List, ListBuilder);
impl_common_layout_props!(List, ListBuilder);

// =============================================================================
// 17. Table (§7.2, §7.3 Required Tier, TypeRef::TABLE / id 18)
// =============================================================================

impl_widget_boilerplate!(
    Table,
    TableBuilder,
    TypeRef::TABLE,
    "Multi-column row-based collection view (§7.2, §7.3 Required Tier, §8)."
);
impl_model_ref_prop!(Table, TableBuilder);
impl_list_prop!(Table, TableBuilder, columns, columns_of, get_columns, set_columns, set_columns_for, op_set_columns, op_set_columns_for, clear_columns, clear_columns_for, op_clear_columns, op_clear_columns_for, PropertyRef::COLUMNS, "Column definitions list (§7.4, §8).");
impl_enum_prop!(Table, TableBuilder, SelectionMode, selection_mode, selection_mode_of, get_selection_mode, set_selection_mode, set_selection_mode_for, op_set_selection_mode, op_set_selection_mode_for, clear_selection_mode, clear_selection_mode_for, op_clear_selection_mode, op_clear_selection_mode_for, PropertyRef::SELECTION_MODE, "Selection mode (§8).");
impl_bool_prop!(Table, TableBuilder, selected, selected_of, get_selected, is_selected, is_selected_of, is_selected_for, set_selected, set_selected_for, op_set_selected, op_set_selected_for, clear_selected, clear_selected_for, op_clear_selected, op_clear_selected_for, PropertyRef::SELECTED, "Selection state (§7.4).");
impl_string_prop!(Table, TableBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Table accessibility label (§7.4).");
impl_string_prop!(Table, TableBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_common_state_props!(Table, TableBuilder);
impl_common_layout_props!(Table, TableBuilder);

// =============================================================================
// 18. Tree (§7.2, §7.3 Required Tier, TypeRef::TREE / id 19)
// =============================================================================

impl_widget_boilerplate!(
    Tree,
    TreeBuilder,
    TypeRef::TREE,
    "Hierarchical outline collection view with expandable nodes (§7.2, §7.3 Required Tier, §8)."
);
impl_model_ref_prop!(Tree, TreeBuilder);
impl_enum_prop!(Tree, TreeBuilder, SelectionMode, selection_mode, selection_mode_of, get_selection_mode, set_selection_mode, set_selection_mode_for, op_set_selection_mode, op_set_selection_mode_for, clear_selection_mode, clear_selection_mode_for, op_clear_selection_mode, op_clear_selection_mode_for, PropertyRef::SELECTION_MODE, "Selection mode (§8).");
impl_bool_prop!(Tree, TreeBuilder, selected, selected_of, get_selected, is_selected, is_selected_of, is_selected_for, set_selected, set_selected_for, op_set_selected, op_set_selected_for, clear_selected, clear_selected_for, op_clear_selected, op_clear_selected_for, PropertyRef::SELECTED, "Selection state (§7.4).");
impl_string_prop!(Tree, TreeBuilder, label, label_of, get_label, set_label, set_label_for, op_set_label, op_set_label_for, clear_label, clear_label_for, op_clear_label, op_clear_label_for, PropertyRef::LABEL, "Tree accessibility label (§7.4).");
impl_string_prop!(Tree, TreeBuilder, accessible_description, accessible_description_of, get_accessible_description, set_accessible_description, set_accessible_description_for, op_set_accessible_description, op_set_accessible_description_for, clear_accessible_description, clear_accessible_description_for, op_clear_accessible_description, op_clear_accessible_description_for, PropertyRef::ACCESSIBLE_DESCRIPTION, "Secondary accessibility description (§7.4).");
impl_common_state_props!(Tree, TreeBuilder);
impl_common_layout_props!(Tree, TreeBuilder);
