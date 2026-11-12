//! Server-side builder for hierarchical collection models (§8, §22.5, §22.7, §26).
//!
//! A `Tree` model's index space is the depth-first preorder of the whole forest. [`TreeModel`]
//! turns an application's forest into those preorder [`ModelItem`]s, stamping each with the
//! standard item properties `tree_depth` (0 for a root) and `descendant_count` (its subtree size,
//! excluding itself), and refuses a forest deeper than the §26 `max_model_tree_depth` limit before
//! anything reaches a transaction. The model operations themselves are unchanged: the items go out
//! through the ordinary `MODEL_RESET_RANGE`/`MODEL_INSERT`/`MODEL_UPDATE`, and the store checks the
//! shape again at commit ([`srui_semantic_tree::Model::validate_tree_shape`]). Replacing a
//! populated model with a smaller forest also deletes the old tail by index range
//! ([`TreeModel::replace_ops`]), because `MODEL_RESET_RANGE` only rewrites its own range.
//!
//! Disclosure is local to the client (§22.5); nothing here carries expansion state.
//!
//! Hierarchy edits should delete by index range rather than by `ItemId`: a sparse replica that
//! does not cache a deleted item cannot shift its other cached indices for an identity delete (§8).

use std::collections::HashSet;
use std::fmt;

use srui_semantic_tree::{
    ItemId, ModelId, ModelItem, Operation, PropertyRef, StoreLimits, Value,
    DEFAULT_MAX_MODEL_TREE_DEPTH,
};

/// One node of an application forest: its item and its ordered children (§8).
#[derive(Debug, Clone, PartialEq)]
pub struct TreeNode {
    /// The item this node contributes; its `tree_depth`/`descendant_count` are overwritten.
    pub item: ModelItem,
    /// Ordered children, rendered after this node in preorder.
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    /// A leaf with the given identity and value.
    pub fn new(item_id: impl Into<ItemId>, value: impl Into<Value>) -> Self {
        Self::from_item(ModelItem::with_value(item_id, value))
    }

    /// A leaf carrying an existing item (column values and other properties are kept).
    pub fn from_item(item: ModelItem) -> Self {
        Self {
            item,
            children: Vec::new(),
        }
    }

    /// Sets one item property (for example a column value).
    pub fn with_property(mut self, property: PropertyRef, value: impl Into<Value>) -> Self {
        self.item.set_property(property, value);
        self
    }

    /// Appends one child.
    pub fn with_child(mut self, child: TreeNode) -> Self {
        self.children.push(child);
        self
    }

    /// Appends several children in order.
    pub fn with_children(mut self, children: impl IntoIterator<Item = TreeNode>) -> Self {
        self.children.extend(children);
        self
    }
}

impl Drop for TreeNode {
    /// Drops descendants iteratively, so an arbitrarily deep forest (for example one refused for
    /// exceeding the depth limit) cannot overflow the stack while it is discarded.
    fn drop(&mut self) {
        let mut stack = std::mem::take(&mut self.children);
        while let Some(mut node) = stack.pop() {
            stack.append(&mut node.children);
        }
    }
}

/// Why a forest cannot become a hierarchical model (§8, §26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeModelError {
    /// An item would sit at `tree_depth >= limit` (the limit counts levels).
    DepthLimitExceeded {
        item_id: ItemId,
        tree_depth: u64,
        limit: usize,
    },
    /// The same `ItemId` appears twice in the forest.
    DuplicateItemId(ItemId),
}

impl fmt::Display for TreeModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DepthLimitExceeded {
                item_id,
                tree_depth,
                limit,
            } => write!(
                f,
                "item {} would sit at tree_depth {}, but max_model_tree_depth is {} levels",
                item_id, tree_depth, limit
            ),
            Self::DuplicateItemId(item_id) => {
                write!(f, "item {} appears twice in the forest", item_id)
            }
        }
    }
}

impl std::error::Error for TreeModelError {}

/// A forest flattened into preorder model items with `tree_depth` and `descendant_count` (§8).
///
/// ```rust
/// # use srui_sdk::*;
/// let forest = vec![
///     TreeNode::new(1u64, "src").with_child(TreeNode::new(2u64, "main.rs")),
///     TreeNode::new(3u64, "Cargo.toml"),
/// ];
/// let items = TreeModel::new(forest).into_items().unwrap();
/// let depths: Vec<_> = items.iter().map(|i| i.get_property(TREE_DEPTH).cloned()).collect();
/// assert_eq!(depths, vec![Some(Value::UnsignedInt(0)), Some(Value::UnsignedInt(1)), Some(Value::UnsignedInt(0))]);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TreeModel {
    roots: Vec<TreeNode>,
    max_depth: usize,
}

impl TreeModel {
    /// A forest bounded by the default `max_model_tree_depth` (256 levels).
    pub fn new(roots: impl IntoIterator<Item = TreeNode>) -> Self {
        Self {
            roots: roots.into_iter().collect(),
            max_depth: DEFAULT_MAX_MODEL_TREE_DEPTH,
        }
    }

    /// Bounds the forest by an explicit level count.
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Bounds the forest by a store's `max_model_tree_depth`, so the builder refuses exactly what
    /// that store would.
    pub fn with_limits(self, limits: &StoreLimits) -> Self {
        self.with_max_depth(limits.max_model_tree_depth)
    }

    /// Flattens the forest into depth-first preorder items (§8).
    ///
    /// Iterative, so an adversarially deep forest is refused at the limit instead of exhausting
    /// the stack. Linear in the number of nodes.
    pub fn into_items(self) -> Result<Vec<ModelItem>, TreeModelError> {
        let max_depth = self.max_depth as u64;
        let mut items: Vec<ModelItem> = Vec::new();
        let mut seen: HashSet<ItemId> = HashSet::new();
        // (node, depth) waiting to be emitted, plus the preorder index of each open ancestor so
        // its descendant count can be filled in once its subtree is complete.
        let mut pending: Vec<(TreeNode, u64)> =
            self.roots.into_iter().rev().map(|n| (n, 0)).collect();
        let mut open: Vec<(usize, u64)> = Vec::new();
        while let Some((mut node, depth)) = pending.pop() {
            close_spans(&mut items, &mut open, depth);
            let children = std::mem::take(&mut node.children);
            let mut item = std::mem::replace(
                &mut node.item,
                ModelItem::with_value(ItemId::new(0), Value::Null),
            );
            if depth >= max_depth {
                return Err(TreeModelError::DepthLimitExceeded {
                    item_id: item.item_id,
                    tree_depth: depth,
                    limit: self.max_depth,
                });
            }
            if !seen.insert(item.item_id) {
                return Err(TreeModelError::DuplicateItemId(item.item_id));
            }
            item.set_property(PropertyRef::TREE_DEPTH, Value::UnsignedInt(depth));
            item.set_property(PropertyRef::DESCENDANT_COUNT, Value::UnsignedInt(0));
            open.push((items.len(), depth));
            items.push(item);
            pending.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
        }
        close_spans(&mut items, &mut open, 0);
        Ok(items)
    }

    /// One `MODEL_RESET_RANGE` writing the forest's preorder from index 0, with `total_count` the
    /// forest size (§8, §13).
    ///
    /// This replaces the whole preorder only when `model` holds no more items than the forest (a
    /// new or growing model): `MODEL_RESET_RANGE` never removes cached items past its range, so a
    /// shrinking reset leaves the old tail cached beyond the new `item_count` and the commit is
    /// refused. An empty forest yields an empty reset, which leaves `item_count` unchanged (a zero
    /// `total_count` means "unchanged" on the wire). To replace a populated model, use
    /// [`Self::replace_ops`].
    pub fn reset_op(self, model: ModelId) -> Result<Operation, TreeModelError> {
        let items = self.into_items()?;
        let total = items.len() as u64;
        Ok(Operation::model_reset_range(model, 0, items, Some(total)))
    }

    /// The operations replacing `model`'s whole preorder, currently `current_item_count` items,
    /// with this forest (§8, §13). Apply them in order, in one transaction.
    ///
    /// When the forest is smaller, a `MODEL_DELETE` of the index range
    /// `forest size .. current_item_count` comes first, so no old item stays cached past the new
    /// `item_count` and an `ItemId` moving out of the old tail is not a duplicate; then one
    /// `MODEL_RESET_RANGE` from index 0 writes the forest (none for an empty forest, so an empty
    /// forest empties the model). Pass the model's current count, for example
    /// `ui.get_model(model).map_or(0, |m| m.item_count())`; a stale count fails the commit
    /// rather than leaving stale items.
    pub fn replace_ops(
        self,
        model: ModelId,
        current_item_count: u64,
    ) -> Result<Vec<Operation>, TreeModelError> {
        let items = self.into_items()?;
        let total = items.len() as u64;
        let mut ops = Vec::with_capacity(2);
        if current_item_count > total {
            ops.push(Operation::model_delete_range(
                model,
                total,
                current_item_count - total,
            ));
        }
        if total > 0 {
            ops.push(Operation::model_reset_range(model, 0, items, Some(total)));
        }
        Ok(ops)
    }
}

/// Pops every open ancestor at `depth` or deeper and records its descendant count.
fn close_spans(items: &mut [ModelItem], open: &mut Vec<(usize, u64)>, depth: u64) {
    let next = items.len();
    while let Some(&(index, open_depth)) = open.last() {
        if open_depth < depth {
            break;
        }
        open.pop();
        let descendants = (next - index - 1) as u64;
        items[index].set_property(
            PropertyRef::DESCENDANT_COUNT,
            Value::UnsignedInt(descendants),
        );
    }
}
