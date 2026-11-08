//! Collection models and sparse cached items for virtualized data views (§8, §13).
//!
//! # Architecture & Protocol Invariants
//!
//! - **§8 Collections and Model Data**: Large collection data is not represented as thousands
//!   of individual child view nodes. Instead, a `List`, `Table`, or `Tree` node references a
//!   `Model` containing stable `ItemId`s, total collection `item_count` (which may be millions),
//!   and a sparse representation of currently cached item ranges.
//! - **§13 Core Mutation Operations**: Collection data is mutated via five standard model operations:
//!   `CREATE_MODEL`, `MODEL_INSERT`, `MODEL_DELETE`, `MODEL_UPDATE`, and `MODEL_RESET_RANGE`.
//! - **Stable Identity**: Items are identified by stable [`ItemId`] values, supporting mutation
//!   by item identity independently of display index or scroll position.
//! - **§8 Hierarchical models, §26 limits**: a `Tree` model's index space is the depth-first
//!   preorder of the whole forest. The standard item properties `tree_depth` and
//!   `descendant_count` carry its shape, and [`Model::validate_tree_shape`] checks the cached items
//!   against it and against `max_model_tree_depth` once per transaction (§12.1). Range requests
//!   and model operations keep flat index semantics.

use crate::ids::{ItemId, ModelId, PropertyRef, TypeRef};
use crate::store::error::{ModelTreeViolation, StoreError};
use crate::value::{Property, Range, Value, ValueConversionError};
use smallvec::SmallVec;
use std::collections::{BTreeMap, HashMap, HashSet};

/// An item within a collection data model (§8, §13).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelItem {
    /// Stable identifier for this item within the model lifetime (§8).
    pub item_id: ItemId,
    /// Primary scalar or record value of the item (§6.5).
    pub value: Value,
    /// Defined properties/attributes on this item (e.g. column values, metadata).
    pub properties: HashMap<PropertyRef, Value>,
}

impl ModelItem {
    /// Constructs a new `ModelItem` with the specified ID, value, and properties.
    pub fn new(
        item_id: impl Into<ItemId>,
        value: impl Into<Value>,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Self {
        Self {
            item_id: item_id.into(),
            value: value.into(),
            properties: properties.into_iter().collect(),
        }
    }

    /// Constructs a new `ModelItem` with a value and empty properties.
    pub fn with_value(item_id: impl Into<ItemId>, value: impl Into<Value>) -> Self {
        Self {
            item_id: item_id.into(),
            value: value.into(),
            properties: HashMap::new(),
        }
    }

    /// Returns a reference to the property value if defined on this item.
    pub fn get_property(&self, prop: PropertyRef) -> Option<&Value> {
        self.properties.get(&prop)
    }

    /// Returns `true` if the item has a defined value for the specified property.
    pub fn has_property(&self, prop: PropertyRef) -> bool {
        self.properties.contains_key(&prop)
    }

    /// Sets a property on this item.
    pub fn set_property(&mut self, prop: PropertyRef, val: impl Into<Value>) {
        self.properties.insert(prop, val.into());
    }

    /// Clears a property from this item, returning the previous value if set.
    pub fn clear_property(&mut self, prop: PropertyRef) -> Option<Value> {
        self.properties.remove(&prop)
    }
}

impl From<&ModelItem> for srui_protocol::ModelItem {
    fn from(item: &ModelItem) -> Self {
        let properties = item
            .properties
            .iter()
            .map(|(&p, v)| srui_protocol::Property {
                property: Some(p.into()),
                value: Some(v.into()),
            })
            .collect();

        srui_protocol::ModelItem {
            item_id: item.item_id.get(),
            value: Some((&item.value).into()),
            properties,
        }
    }
}

impl From<ModelItem> for srui_protocol::ModelItem {
    fn from(item: ModelItem) -> Self {
        (&item).into()
    }
}

impl TryFrom<srui_protocol::ModelItem> for ModelItem {
    type Error = ValueConversionError;

    fn try_from(wire: srui_protocol::ModelItem) -> Result<Self, Self::Error> {
        let item_id = ItemId::new(wire.item_id);
        let value = match wire.value {
            Some(v) => Value::try_from(v)?,
            None => Value::Null,
        };

        let mut properties = HashMap::with_capacity(wire.properties.len());
        for p in wire.properties {
            let prop = Property::try_from(p)?;
            properties.insert(prop.property, prop.value);
        }

        Ok(Self {
            item_id,
            value,
            properties,
        })
    }
}

/// An authoritative collection data model (§8, §13).
///
/// Models maintain:
/// 1. `item_count`: Total logical items in the remote collection (e.g. 500,000).
/// 2. `items`: Sparse cached item ranges indexed by position (`0..item_count`).
/// 3. `id_to_index`: Reverse lookup index mapping stable `ItemId` to current index position.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// Unique identifier for this model within the session (§6.2, §8).
    pub id: ModelId,
    /// Semantic type reference for this model (e.g. List, Table, Tree, or custom).
    pub model_type: TypeRef,
    /// Total logical item count in the collection (may far exceed cached item count, §8).
    pub item_count: u64,
    /// Sparse map of cached items keyed by their 0-based collection index.
    pub items: BTreeMap<u64, ModelItem>,
    /// Reverse lookup mapping each cached item's stable `ItemId` to its index.
    pub id_to_index: HashMap<ItemId, u64>,
    /// Set once any item carrying `tree_depth` or `descendant_count` has been written, and kept
    /// for the model's lifetime: a model that never held one is a flat list of roots and needs no
    /// shape check (§8).
    pub tree_shaped: bool,
}

/// Returns `true` when `item` carries one of the hierarchical shape properties (§8).
fn carries_tree_shape(item: &ModelItem) -> bool {
    item.properties.contains_key(&PropertyRef::TREE_DEPTH)
        || item.properties.contains_key(&PropertyRef::DESCENDANT_COUNT)
}

/// Reads one hierarchical shape property; absent is 0, and only a non-negative integer is a
/// shape (§8).
fn tree_shape_value(item: &ModelItem, property: PropertyRef) -> Result<u64, ModelTreeViolation> {
    match item.properties.get(&property) {
        None => Ok(0),
        Some(Value::UnsignedInt(value)) => Ok(*value),
        Some(Value::SignedInt(value)) if *value >= 0 => Ok(*value as u64),
        Some(_) => Err(ModelTreeViolation::InvalidProperty {
            property: property.standard_name().unwrap_or("tree property"),
        }),
    }
}

impl Model {
    /// Constructs a new empty collection model with specified item count (§8, §13).
    pub fn new(id: ModelId, model_type: TypeRef, item_count: u64) -> Self {
        Self {
            id,
            model_type,
            item_count,
            items: BTreeMap::new(),
            id_to_index: HashMap::new(),
            tree_shaped: false,
        }
    }

    fn note_tree_shape(&mut self, items: &[ModelItem]) {
        if !self.tree_shaped && items.iter().any(carries_tree_shape) {
            self.tree_shaped = true;
        }
    }

    /// Checks the cached items against the preorder shape their `tree_depth` and
    /// `descendant_count` declare, and against `max_model_tree_depth` (§8, §26).
    ///
    /// Items are visited in index order and the first violation is reported, so the Swift replica
    /// and this store reject the same cache identically. Per item, in order:
    ///
    /// 1. both properties are non-negative integers (absent is 0);
    /// 2. `tree_depth < max_model_tree_depth`;
    /// 3. `index + descendant_count < item_count`;
    /// 4. index 0 has depth 0, and a cached item at `index - 1` is at most one level shallower;
    /// 5. an item directly after a cached item's span is no deeper than that item, and an item
    ///    inside the nearest enclosing cached span is deeper than its owner and ends inside it.
    ///
    /// Only cached items are compared: an uncached neighbour is never guessed at, so a sparse
    /// replica accepts what the authoritative store accepts for the same cache (§6.3).
    ///
    /// lc-debt: O(cached items) per touched hierarchical model per transaction; upgrade path is
    /// a check windowed around the touched index ranges plus a max-span index for rule 3.
    pub fn validate_tree_shape(&self, max_model_tree_depth: usize) -> Result<(), StoreError> {
        if !self.tree_shaped {
            return Ok(());
        }
        let fail = |index: u64, violation: ModelTreeViolation| StoreError::InvalidModelTree {
            model_id: self.id,
            index,
            violation,
        };
        // Open spans of cached items as (index, depth, last index of the span). Each entry is
        // strictly deeper than the one below it, so the stack never exceeds the depth limit.
        let mut open: Vec<(u64, u64, u64)> = Vec::new();
        let mut previous: Option<(u64, u64)> = None;
        for (&index, item) in &self.items {
            let depth =
                tree_shape_value(item, PropertyRef::TREE_DEPTH).map_err(|v| fail(index, v))?;
            let descendants = tree_shape_value(item, PropertyRef::DESCENDANT_COUNT)
                .map_err(|v| fail(index, v))?;
            if depth >= max_model_tree_depth as u64 {
                return Err(StoreError::MaxModelTreeDepthExceeded {
                    model_id: self.id,
                    index,
                    limit: max_model_tree_depth,
                    tree_depth: depth,
                });
            }
            if descendants >= self.item_count.saturating_sub(index) {
                return Err(fail(
                    index,
                    ModelTreeViolation::DescendantCountOutOfRange {
                        descendant_count: descendants,
                        item_count: self.item_count,
                    },
                ));
            }
            let previous_depth = match previous {
                Some((prev_index, prev_depth)) if prev_index + 1 == index => Some(prev_depth),
                _ => None,
            };
            if (index == 0 && depth != 0)
                || previous_depth.is_some_and(|prev| depth > prev.saturating_add(1))
            {
                return Err(fail(
                    index,
                    ModelTreeViolation::DepthJump {
                        tree_depth: depth,
                        previous_depth,
                    },
                ));
            }
            while let Some(&(owner, owner_depth, end)) = open.last() {
                if end >= index {
                    break;
                }
                if end + 1 == index && depth > owner_depth {
                    return Err(fail(
                        index,
                        ModelTreeViolation::OutsideParentSpan {
                            parent_index: owner,
                        },
                    ));
                }
                open.pop();
            }
            // `descendants < item_count - index`, so the span end cannot overflow.
            let end = index + descendants;
            if let Some(&(owner, owner_depth, owner_end)) = open.last() {
                if depth <= owner_depth || end > owner_end {
                    return Err(fail(
                        index,
                        ModelTreeViolation::OutsideParentSpan {
                            parent_index: owner,
                        },
                    ));
                }
            }
            if descendants > 0 {
                open.push((index, depth, end));
            }
            previous = Some((index, depth));
        }
        Ok(())
    }

    /// Constructs a model pre-populated with an initial contiguous range of items.
    pub fn with_items(
        id: ModelId,
        model_type: TypeRef,
        item_count: u64,
        start_index: u64,
        items: impl IntoIterator<Item = ModelItem>,
    ) -> Result<Self, StoreError> {
        let mut model = Self::new(id, model_type, item_count);
        let item_list: Vec<ModelItem> = items.into_iter().collect();
        model.reset_range(start_index, item_list, None)?;
        Ok(model)
    }

    /// Returns the total logical number of items in the collection (§8).
    pub fn item_count(&self) -> u64 {
        self.item_count
    }

    /// Updates the total logical item count of the collection.
    pub fn set_item_count(&mut self, count: u64) {
        self.item_count = count;
    }

    /// Returns the number of currently cached items in memory.
    pub fn cached_item_count(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if the model has zero total items.
    pub fn is_empty(&self) -> bool {
        self.item_count == 0
    }

    /// Returns `true` if the given `ItemId` is present in the cache.
    pub fn contains_item(&self, item_id: ItemId) -> bool {
        self.id_to_index.contains_key(&item_id)
    }

    /// Returns `true` if the given index is currently cached.
    pub fn contains_index(&self, index: u64) -> bool {
        self.items.contains_key(&index)
    }

    /// Drops up to `count` cached items whose indices lie outside `[keep_start, keep_end)`.
    ///
    /// Farthest from the keep-window midpoint are removed first; equal distances prefer the
    /// lower index so server and replica eviction stays deterministic (§8, §26).
    pub fn evict_farthest_outside(&mut self, keep_start: u64, keep_end: u64, count: usize) {
        if count == 0 {
            return;
        }
        let keep_end = keep_end.max(keep_start);
        let mid = keep_start.saturating_add(keep_end.saturating_sub(keep_start) / 2);
        let mut candidates: Vec<u64> = self
            .items
            .keys()
            .copied()
            .filter(|&index| index < keep_start || index >= keep_end)
            .collect();
        candidates.sort_by(|a, b| {
            let da = a.abs_diff(mid);
            let db = b.abs_diff(mid);
            db.cmp(&da).then(a.cmp(b))
        });
        for index in candidates.into_iter().take(count) {
            if let Some(item) = self.items.remove(&index) {
                self.id_to_index.remove(&item.item_id);
            }
        }
    }

    /// Returns the current index for a cached `ItemId`, if present.
    pub fn index_of(&self, item_id: ItemId) -> Option<u64> {
        self.id_to_index.get(&item_id).copied()
    }

    /// Returns a reference to a cached item by its stable `ItemId`.
    pub fn get_item_by_id(&self, item_id: ItemId) -> Option<&ModelItem> {
        let idx = self.id_to_index.get(&item_id)?;
        self.items.get(idx)
    }

    /// Returns a mutable reference to a cached item by its stable `ItemId`.
    pub fn get_item_mut_by_id(&mut self, item_id: ItemId) -> Option<&mut ModelItem> {
        let idx = *self.id_to_index.get(&item_id)?;
        self.items.get_mut(&idx)
    }

    /// Returns a reference to a cached item by its 0-based collection index.
    pub fn get_item_by_index(&self, index: u64) -> Option<&ModelItem> {
        self.items.get(&index)
    }

    /// Returns a mutable reference to a cached item by its 0-based collection index.
    pub fn get_item_mut_by_index(&mut self, index: u64) -> Option<&mut ModelItem> {
        self.items.get_mut(&index)
    }

    /// Returns an iterator over all currently cached (index, item) pairs in ascending index order.
    pub fn iter_cached_items(&self) -> impl Iterator<Item = (&u64, &ModelItem)> {
        self.items.iter()
    }

    /// Computes the list of contiguous index ranges currently cached in memory (§8).
    pub fn cached_ranges(&self) -> Vec<Range> {
        let mut ranges = Vec::new();
        let mut current_range: Option<Range> = None;

        for &idx in self.items.keys() {
            match current_range.as_mut() {
                Some(r) if r.start + r.length == idx => {
                    r.length += 1;
                }
                Some(r) => {
                    ranges.push(*r);
                    current_range = Some(Range::new(idx, 1));
                }
                None => {
                    current_range = Some(Range::new(idx, 1));
                }
            }
        }

        if let Some(r) = current_range {
            ranges.push(r);
        }

        ranges
    }

    /// Shifts all cached item indices starting at `>= from_index` by signed `delta`.
    fn shift_cached_indices(&mut self, from_index: u64, delta: i64) {
        if delta == 0 {
            return;
        }
        let to_shift = self.items.split_off(&from_index);
        for (old_idx, item) in to_shift {
            let new_idx = if delta > 0 {
                old_idx + delta as u64
            } else {
                old_idx - (-delta) as u64
            };
            self.id_to_index.insert(item.item_id, new_idx);
            self.items.insert(new_idx, item);
        }
    }

    /// Inserts items into the model at the specified index, shifting subsequent items (§13 MODEL_INSERT).
    pub fn insert_items(&mut self, index: u64, items: Vec<ModelItem>) -> Result<(), StoreError> {
        if index > self.item_count {
            return Err(StoreError::ModelIndexOutOfBounds {
                index,
                count: self.item_count,
            });
        }

        // Validate uniqueness of ItemIds
        let mut incoming_ids = HashSet::with_capacity(items.len());
        for item in &items {
            if self.contains_item(item.item_id) || !incoming_ids.insert(item.item_id) {
                return Err(StoreError::DuplicateItemId {
                    model_id: self.id,
                    item_id: item.item_id,
                });
            }
        }

        self.note_tree_shape(&items);
        let n = items.len() as u64;
        if n > 0 {
            self.shift_cached_indices(index, n as i64);

            // Insert new items
            for (i, item) in items.into_iter().enumerate() {
                let target_idx = index + i as u64;
                self.id_to_index.insert(item.item_id, target_idx);
                self.items.insert(target_idx, item);
            }

            self.item_count += n;
        }

        Ok(())
    }

    /// Deletes items by `item_ids` or by index range (§13 MODEL_DELETE).
    ///
    /// Per §8 and §13, a `MODEL_DELETE` operation must specify either discrete item identities
    /// (`item_ids`) or a contiguous index range (`index` + `count`), but not both.
    /// Attempting to combine both selectors returns [`StoreError::InvalidModelDelete`] to prevent
    /// double-decrement corruption of `item_count`.
    ///
    /// Note (§8 Sparse Collection Invariant): When deleting by `item_id`, if the item is not
    /// currently held in the local sparse cache (uncached), `item_count` is still decremented
    /// to maintain consistency with the authoritative collection length.
    pub fn delete_items(
        &mut self,
        index: Option<u64>,
        count: Option<u64>,
        item_ids: &[ItemId],
    ) -> Result<(), StoreError> {
        let has_range = count.is_some() && count.unwrap() > 0;
        let has_ids = !item_ids.is_empty();

        if has_range && has_ids {
            return Err(StoreError::InvalidModelDelete(
                "cannot combine item_ids and range deletion in ModelDelete (§8, §13)".to_string(),
            ));
        }

        if has_ids {
            for &item_id in item_ids {
                if let Some(idx) = self.id_to_index.remove(&item_id) {
                    self.items.remove(&idx);
                    self.item_count = self.item_count.saturating_sub(1);
                    self.shift_cached_indices(idx + 1, -1);
                } else {
                    self.item_count = self.item_count.saturating_sub(1);
                }
            }
            return Ok(());
        }

        if has_range {
            let idx = index.unwrap_or(0);
            let cnt = count.unwrap();

            if idx > self.item_count || idx.saturating_add(cnt) > self.item_count {
                return Err(StoreError::ModelIndexOutOfBounds {
                    index: idx.saturating_add(cnt),
                    count: self.item_count,
                });
            }

            // Remove only cached items within the deleted range in O(k log N)
            if self.items.range(idx..idx + cnt).next().is_some() {
                let cached_keys_in_range: SmallVec<[u64; 16]> =
                    self.items.range(idx..idx + cnt).map(|(&k, _)| k).collect();
                for k in cached_keys_in_range {
                    if let Some(removed) = self.items.remove(&k) {
                        self.id_to_index.remove(&removed.item_id);
                    }
                }
            }

            self.shift_cached_indices(idx + cnt, -(cnt as i64));
            self.item_count = self.item_count.saturating_sub(cnt);
            return Ok(());
        }

        Ok(())
    }

    /// Updates existing items by identity or index without shifting (§13 MODEL_UPDATE).
    ///
    /// # Conflict Resolution & Semantics
    ///
    /// - **Positional Update (`index: Some(base_idx)`)**: Items are written directly to indices
    ///   `base_idx + i`. If an incoming `item_id` was previously cached at a different index position,
    ///   its old index entry is removed and re-keyed to the new target index, ensuring bidirectional
    ///   `id_to_index` and `items` consistency without duplicating items.
    /// - **Identity Update (`index: None`)**: Items are matched by stable `item_id` in `id_to_index`.
    ///   If an item is not found in the sparse cache, [`StoreError::ItemNotFound`] is returned.
    pub fn update_items(
        &mut self,
        index: Option<u64>,
        items: Vec<ModelItem>,
    ) -> Result<(), StoreError> {
        self.note_tree_shape(&items);
        for (i, item) in items.into_iter().enumerate() {
            if let Some(base_idx) = index {
                let target_idx = base_idx + i as u64;
                if target_idx >= self.item_count {
                    return Err(StoreError::ModelIndexOutOfBounds {
                        index: target_idx,
                        count: self.item_count,
                    });
                }
                // If item_id was previously cached at another index, remove its old slot
                if let Some(old_pos) = self.id_to_index.remove(&item.item_id) {
                    if old_pos != target_idx {
                        self.items.remove(&old_pos);
                    }
                }
                // If target_idx currently holds a different item, remove its id_to_index entry
                if let Some(displaced) = self.items.remove(&target_idx) {
                    self.id_to_index.remove(&displaced.item_id);
                }
                self.id_to_index.insert(item.item_id, target_idx);
                self.items.insert(target_idx, item);
            } else if let Some(&idx) = self.id_to_index.get(&item.item_id) {
                self.items.insert(idx, item);
            } else {
                return Err(StoreError::ItemNotFound(item.item_id));
            }
        }
        Ok(())
    }

    /// Resets/replaces a contiguous range of cached items (§13 MODEL_RESET_RANGE).
    pub fn reset_range(
        &mut self,
        start_index: u64,
        items: Vec<ModelItem>,
        total_count: Option<u64>,
    ) -> Result<(), StoreError> {
        if let Some(tc) = total_count {
            if tc > 0 {
                self.item_count = tc;
            }
        }

        if start_index > self.item_count {
            return Err(StoreError::ModelIndexOutOfBounds {
                index: start_index,
                count: self.item_count,
            });
        }

        let n = items.len() as u64;
        let end_index = start_index + n;
        if end_index > self.item_count {
            return Err(StoreError::ModelIndexOutOfBounds {
                index: end_index,
                count: self.item_count,
            });
        }

        // Validate incoming ItemIds for duplicates within items and collisions outside range
        let mut incoming_ids = HashSet::with_capacity(items.len());
        for item in &items {
            if !incoming_ids.insert(item.item_id) {
                return Err(StoreError::DuplicateItemId {
                    model_id: self.id,
                    item_id: item.item_id,
                });
            }
            if let Some(&existing_idx) = self.id_to_index.get(&item.item_id) {
                if existing_idx < start_index || existing_idx >= end_index {
                    return Err(StoreError::DuplicateItemId {
                        model_id: self.id,
                        item_id: item.item_id,
                    });
                }
            }
        }

        self.note_tree_shape(&items);
        // Remove old cached items in [start_index, end_index)
        for idx in start_index..end_index {
            if let Some(old) = self.items.remove(&idx) {
                self.id_to_index.remove(&old.item_id);
            }
        }

        // Insert new items
        for (i, item) in items.into_iter().enumerate() {
            let idx = start_index + i as u64;
            self.id_to_index.insert(item.item_id, idx);
            self.items.insert(idx, item);
        }

        Ok(())
    }
}
