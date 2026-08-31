//! Internal declarative macros for widget handle and builder generation (§7.1–§7.5).

#[macro_export]
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
            pub fn create(self, store: &mut impl StoreMut) -> Result<$widget, StoreError> {
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
            pub fn $setter(&self, store: &mut impl StoreMut, value: impl Into<String>) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut impl StoreMut, id: NodeId, value: impl Into<String>) -> Result<Option<Value>, StoreError> {
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
            pub fn $clear(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut impl StoreMut, id: NodeId) -> Result<Option<Value>, StoreError> {
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
            pub fn $setter(&self, store: &mut impl StoreMut, value: bool) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut impl StoreMut, id: NodeId, value: bool) -> Result<Option<Value>, StoreError> {
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
            pub fn $clear(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut impl StoreMut, id: NodeId) -> Result<Option<Value>, StoreError> {
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
            pub fn $setter(&self, store: &mut impl StoreMut, value: $enum_type) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut impl StoreMut, id: NodeId, value: $enum_type) -> Result<Option<Value>, StoreError> {
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
            pub fn $clear(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut impl StoreMut, id: NodeId) -> Result<Option<Value>, StoreError> {
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
            pub fn $setter(&self, store: &mut impl StoreMut, value: f64) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut impl StoreMut, id: NodeId, value: f64) -> Result<Option<Value>, StoreError> {
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
            pub fn $clear(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut impl StoreMut, id: NodeId) -> Result<Option<Value>, StoreError> {
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
            pub fn $setter(&self, store: &mut impl StoreMut, value: Size) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut impl StoreMut, id: NodeId, value: Size) -> Result<Option<Value>, StoreError> {
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
            pub fn $clear(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut impl StoreMut, id: NodeId) -> Result<Option<Value>, StoreError> {
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
            pub fn $setter(&self, store: &mut impl StoreMut, value: impl IntoIterator<Item = impl Into<Value>>) -> Result<Option<Value>, StoreError> {
                Self::$setter_for(store, self.id, value)
            }

            #[doc = concat!("Sets the `", stringify!($getter), "` list property on the given node in `store` (§13 SET_PROPERTY).")]
            #[inline]
            pub fn $setter_for(store: &mut impl StoreMut, id: NodeId, value: impl IntoIterator<Item = impl Into<Value>>) -> Result<Option<Value>, StoreError> {
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
            pub fn $clear(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
                Self::$clear_for(store, self.id)
            }

            #[doc = concat!("Clears the `", stringify!($getter), "` property on the given node in `store` (§13 CLEAR_PROPERTY).")]
            #[inline]
            pub fn $clear_for(store: &mut impl StoreMut, id: NodeId) -> Result<Option<Value>, StoreError> {
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
            pub fn set_model_ref(&self, store: &mut impl StoreMut, model_id: impl Into<ModelId>) -> Result<Option<Value>, StoreError> {
                Self::set_model_ref_for(store, self.id, model_id)
            }

            #[doc = "Sets the `model_ref` property on the given node in `store` (§13 SET_PROPERTY, §8)."]
            #[inline]
            pub fn set_model_ref_for(store: &mut impl StoreMut, id: NodeId, model_id: impl Into<ModelId>) -> Result<Option<Value>, StoreError> {
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
            pub fn clear_model_ref(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
                Self::clear_model_ref_for(store, self.id)
            }

            #[doc = "Clears the `model_ref` property on the given node in `store` (§13 CLEAR_PROPERTY)."]
            #[inline]
            pub fn clear_model_ref_for(store: &mut impl StoreMut, id: NodeId) -> Result<Option<Value>, StoreError> {
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
        impl_enum_prop!(
            $widget,
            $builder,
            HorizontalAlignment,
            horizontal_alignment,
            horizontal_alignment_of,
            get_horizontal_alignment,
            set_horizontal_alignment,
            set_horizontal_alignment_for,
            op_set_horizontal_alignment,
            op_set_horizontal_alignment_for,
            clear_horizontal_alignment,
            clear_horizontal_alignment_for,
            op_clear_horizontal_alignment,
            op_clear_horizontal_alignment_for,
            PropertyRef::HORIZONTAL_ALIGNMENT,
            "Horizontal alignment within parent layout (§7.4)."
        );
        impl_enum_prop!(
            $widget,
            $builder,
            VerticalAlignment,
            vertical_alignment,
            vertical_alignment_of,
            get_vertical_alignment,
            set_vertical_alignment,
            set_vertical_alignment_for,
            op_set_vertical_alignment,
            op_set_vertical_alignment_for,
            clear_vertical_alignment,
            clear_vertical_alignment_for,
            op_clear_vertical_alignment,
            op_clear_vertical_alignment_for,
            PropertyRef::VERTICAL_ALIGNMENT,
            "Vertical alignment within parent layout (§7.4)."
        );
        impl_f64_prop!(
            $widget,
            $builder,
            grow,
            grow_of,
            get_grow,
            set_grow,
            set_grow_for,
            op_set_grow,
            op_set_grow_for,
            clear_grow,
            clear_grow_for,
            op_clear_grow,
            op_clear_grow_for,
            PropertyRef::GROW,
            "Relative flex grow weight (§7.4)."
        );
        impl_f64_prop!(
            $widget,
            $builder,
            shrink,
            shrink_of,
            get_shrink,
            set_shrink,
            set_shrink_for,
            op_set_shrink,
            op_set_shrink_for,
            clear_shrink,
            clear_shrink_for,
            op_clear_shrink,
            op_clear_shrink_for,
            PropertyRef::SHRINK,
            "Relative flex shrink weight (§7.4)."
        );
        impl_size_prop!(
            $widget,
            $builder,
            preferred_size,
            preferred_size_of,
            get_preferred_size,
            set_preferred_size,
            set_preferred_size_for,
            op_set_preferred_size,
            op_set_preferred_size_for,
            clear_preferred_size,
            clear_preferred_size_for,
            op_clear_preferred_size,
            op_clear_preferred_size_for,
            PropertyRef::PREFERRED_SIZE,
            "Preferred logical dimensions (§7.4)."
        );
        impl_size_prop!(
            $widget,
            $builder,
            minimum_size,
            minimum_size_of,
            get_minimum_size,
            set_minimum_size,
            set_minimum_size_for,
            op_set_minimum_size,
            op_set_minimum_size_for,
            clear_minimum_size,
            clear_minimum_size_for,
            op_clear_minimum_size,
            op_clear_minimum_size_for,
            PropertyRef::MINIMUM_SIZE,
            "Minimum logical dimensions (§7.4)."
        );
        impl_size_prop!(
            $widget,
            $builder,
            maximum_size,
            maximum_size_of,
            get_maximum_size,
            set_maximum_size,
            set_maximum_size_for,
            op_set_maximum_size,
            op_set_maximum_size_for,
            clear_maximum_size,
            clear_maximum_size_for,
            op_clear_maximum_size,
            op_clear_maximum_size_for,
            PropertyRef::MAXIMUM_SIZE,
            "Maximum logical dimensions (§7.4)."
        );
    };
}

macro_rules! impl_common_state_props {
    ($widget:ident, $builder:ident) => {
        impl_enum_prop!(
            $widget,
            $builder,
            Visibility,
            visibility,
            visibility_of,
            get_visibility,
            set_visibility,
            set_visibility_for,
            op_set_visibility,
            op_set_visibility_for,
            clear_visibility,
            clear_visibility_for,
            op_clear_visibility,
            op_clear_visibility_for,
            PropertyRef::VISIBILITY,
            "Visibility and layout participation (§7.4)."
        );
        impl_bool_prop!(
            $widget,
            $builder,
            enabled,
            enabled_of,
            get_enabled,
            is_enabled,
            is_enabled_of,
            is_enabled_for,
            set_enabled,
            set_enabled_for,
            op_set_enabled,
            op_set_enabled_for,
            clear_enabled,
            clear_enabled_for,
            op_clear_enabled,
            op_clear_enabled_for,
            PropertyRef::ENABLED,
            "Whether the node is interactive (§7.4)."
        );
        impl_bool_prop!(
            $widget,
            $builder,
            busy,
            busy_of,
            get_busy,
            is_busy,
            is_busy_of,
            is_busy_for,
            set_busy,
            set_busy_for,
            op_set_busy,
            op_set_busy_for,
            clear_busy,
            clear_busy_for,
            op_clear_busy,
            op_clear_busy_for,
            PropertyRef::BUSY,
            "Whether the node is performing an asynchronous operation (§7.4)."
        );
    };
}

macro_rules! impl_container_spacing_props {
    ($widget:ident, $builder:ident) => {
        impl_enum_prop!(
            $widget,
            $builder,
            SpacingRole,
            spacing_role,
            spacing_role_of,
            get_spacing_role,
            set_spacing_role,
            set_spacing_role_for,
            op_set_spacing_role,
            op_set_spacing_role_for,
            clear_spacing_role,
            clear_spacing_role_for,
            op_clear_spacing_role,
            op_clear_spacing_role_for,
            PropertyRef::SPACING_ROLE,
            "Semantic inter-item spacing (§7.4)."
        );
        impl_enum_prop!(
            $widget,
            $builder,
            PaddingRole,
            padding_role,
            padding_role_of,
            get_padding_role,
            set_padding_role,
            set_padding_role_for,
            op_set_padding_role,
            op_set_padding_role_for,
            clear_padding_role,
            clear_padding_role_for,
            op_clear_padding_role,
            op_clear_padding_role_for,
            PropertyRef::PADDING_ROLE,
            "Semantic container padding (§7.4)."
        );
    };
}
