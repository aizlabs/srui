use crate::ids::{ItemId, ModelId, NodeId, PropertyRef, TypeRef};
use crate::value::Value;
use std::collections::HashMap;

/// A node within the authoritative semantic graph (§6.2, §6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Unique identifier for this node within the session (§6.2).
    pub id: NodeId,
    /// Type reference of this node (§6.4, §7.2).
    pub node_type: TypeRef,
    /// Parent node ID in the hierarchy (`None` for root nodes).
    pub parent_id: Option<NodeId>,
    /// Strictly ordered sequence of child node IDs (§6.2).
    pub ordered_children: Vec<NodeId>,
    /// Sparse map of defined property values (§7.4, §7.6).
    pub properties: HashMap<PropertyRef, Value>,
}

impl Node {
    /// Constructs a new node with the specified identity, type, parent, and initial properties.
    pub fn new(
        id: NodeId,
        node_type: TypeRef,
        parent_id: Option<NodeId>,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Self {
        Self {
            id,
            node_type,
            parent_id,
            ordered_children: Vec::new(),
            properties: properties.into_iter().collect(),
        }
    }

    /// Returns a reference to the property value if defined on this node.
    pub fn get_property(&self, prop: PropertyRef) -> Option<&Value> {
        self.properties.get(&prop)
    }

    /// Returns `true` if the node has a defined value for the specified property.
    pub fn has_property(&self, prop: PropertyRef) -> bool {
        self.properties.contains_key(&prop)
    }

    /// Returns an iterator over all defined properties on this node.
    pub fn iter_properties(&self) -> impl Iterator<Item = (&PropertyRef, &Value)> {
        self.properties.iter()
    }

    /// Returns the server-promoted selection of a `List`, `Table` or `Tree` (§6.3, §8).
    ///
    /// A collection's standard `value` property, when present, is an [`ItemId`] the server has
    /// explicitly promoted from local presentation state to semantic state. It names an item
    /// identity, never a row index. Other node types, and a collection without `value`, return
    /// `None`.
    pub fn promoted_selection(&self) -> Option<ItemId> {
        if !super::is_selectable_collection(self.node_type) {
            return None;
        }
        self.get_property(PropertyRef::VALUE)
            .and_then(Value::as_item_id)
    }

    /// Returns the server-hinted keep-in-view item of a `List`, `Table` or `Tree` (§6.3, §8).
    ///
    /// The standard `keep_visible` property, when present, names an [`ItemId`] the client keeps in
    /// view. It is presentation guidance and never selection state. Other node types, and a
    /// collection without it, return `None`.
    pub fn keep_visible(&self) -> Option<ItemId> {
        if !super::is_selectable_collection(self.node_type) {
            return None;
        }
        self.get_property(PropertyRef::KEEP_VISIBLE)
            .and_then(Value::as_item_id)
    }

    /// Returns the referenced `ModelId` if this node has a `model_ref` property defined (§8).
    pub fn model_ref(&self) -> Option<ModelId> {
        self.get_property(PropertyRef::MODEL_REF)
            .and_then(|v| match v {
                Value::UnsignedInt(u) => Some(ModelId::new(*u)),
                Value::SignedInt(i) if *i >= 0 => Some(ModelId::new(*i as u64)),
                _ => None,
            })
    }
}
