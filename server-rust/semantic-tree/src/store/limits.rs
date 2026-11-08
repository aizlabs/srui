use super::error::StoreError;
use crate::value::Value;

/// Default maximum allowed operations in a single transaction (§26).
pub const DEFAULT_MAX_TRANSACTION_OPERATIONS: usize = 10_000;
/// Default maximum allowed active models in the store (§26).
pub const DEFAULT_MAX_MODEL_COUNT: usize = 1_000;
/// Default maximum allowed cached items per collection model (§26).
pub const DEFAULT_MAX_CACHED_ITEMS_PER_MODEL: usize = 100_000;
/// Default maximum allowed items in a single model mutation operation (§26).
pub const DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION: usize = 10_000;
/// Default maximum number of levels in a hierarchical collection model (§8, §26): an item's
/// `tree_depth` must stay below it, so roots are depth 0 and the deepest item is depth 255.
pub const DEFAULT_MAX_MODEL_TREE_DEPTH: usize = 256;

/// Configurable mandatory runtime safety limits for `SemanticStore` (§26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreLimits {
    /// Maximum allowed depth of any node in the hierarchy (root is depth 1). Default: 64.
    pub max_tree_depth: usize,
    /// Maximum total active nodes allowed in the store. Default: 100,000.
    pub max_node_count: usize,
    /// Maximum allowed length of a UTF-8 string property in bytes. Default: 1,048,576 (1 MiB).
    pub max_string_length: usize,
    /// Maximum allowed recursion depth for nested values (lists, records). Default: 16.
    pub max_value_depth: usize,
    /// Maximum allowed element count in a single `Value::List`. Default: 10,000.
    pub max_list_elements: usize,
    /// Maximum allowed property count in a single `SmallRecord`. Default: 1,000.
    pub max_record_properties: usize,
    /// Maximum allowed mutation operations in a single transaction. Default: 10,000.
    pub max_transaction_operations: usize,
    /// Maximum allowed active models in the store (§26). Default: 1,000.
    pub max_model_count: usize,
    /// Maximum allowed cached items in a single collection model (§26). Default: 100,000.
    pub max_cached_items_per_model: usize,
    /// Maximum allowed items in a single model mutation batch (§26). Default: 10,000.
    pub max_items_per_model_operation: usize,
    /// Maximum number of levels in a hierarchical collection model (§8, §26): every cached item's
    /// `tree_depth` must be below it. Unlike `max_tree_depth`, which bounds the semantic node
    /// tree, this bounds model data. Default: 256.
    pub max_model_tree_depth: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            max_tree_depth: 64,
            max_node_count: 100_000,
            max_string_length: 1024 * 1024,
            max_value_depth: 16,
            max_list_elements: 10_000,
            max_record_properties: 1_000,
            max_transaction_operations: DEFAULT_MAX_TRANSACTION_OPERATIONS,
            max_model_count: DEFAULT_MAX_MODEL_COUNT,
            max_cached_items_per_model: DEFAULT_MAX_CACHED_ITEMS_PER_MODEL,
            max_items_per_model_operation: DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
            max_model_tree_depth: DEFAULT_MAX_MODEL_TREE_DEPTH,
        }
    }
}

impl StoreLimits {
    /// Creates a new `StoreLimits` with specified basic tree limits and default nested value & model limits.
    pub const fn new(
        max_tree_depth: usize,
        max_node_count: usize,
        max_string_length: usize,
    ) -> Self {
        Self {
            max_tree_depth,
            max_node_count,
            max_string_length,
            max_value_depth: 16,
            max_list_elements: 10_000,
            max_record_properties: 1_000,
            max_transaction_operations: DEFAULT_MAX_TRANSACTION_OPERATIONS,
            max_model_count: DEFAULT_MAX_MODEL_COUNT,
            max_cached_items_per_model: DEFAULT_MAX_CACHED_ITEMS_PER_MODEL,
            max_items_per_model_operation: DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
            max_model_tree_depth: DEFAULT_MAX_MODEL_TREE_DEPTH,
        }
    }

    /// Creates a new `StoreLimits` with customization of basic tree and nested value limits (§26),
    /// using standard defaults for transaction and model limits.
    pub const fn with_tree_and_value_limits(
        max_tree_depth: usize,
        max_node_count: usize,
        max_string_length: usize,
        max_value_depth: usize,
        max_list_elements: usize,
        max_record_properties: usize,
    ) -> Self {
        Self {
            max_tree_depth,
            max_node_count,
            max_string_length,
            max_value_depth,
            max_list_elements,
            max_record_properties,
            max_transaction_operations: DEFAULT_MAX_TRANSACTION_OPERATIONS,
            max_model_count: DEFAULT_MAX_MODEL_COUNT,
            max_cached_items_per_model: DEFAULT_MAX_CACHED_ITEMS_PER_MODEL,
            max_items_per_model_operation: DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
            max_model_tree_depth: DEFAULT_MAX_MODEL_TREE_DEPTH,
        }
    }

    /// Creates a new `StoreLimits` with explicit customization of the 10 store, value, transaction,
    /// and model item limits (§26); `max_model_tree_depth` keeps its default (see
    /// [`Self::with_max_model_tree_depth`]).
    #[allow(clippy::too_many_arguments)]
    pub const fn with_all_limits(
        max_tree_depth: usize,
        max_node_count: usize,
        max_string_length: usize,
        max_value_depth: usize,
        max_list_elements: usize,
        max_record_properties: usize,
        max_transaction_operations: usize,
        max_model_count: usize,
        max_cached_items_per_model: usize,
        max_items_per_model_operation: usize,
    ) -> Self {
        Self {
            max_tree_depth,
            max_node_count,
            max_string_length,
            max_value_depth,
            max_list_elements,
            max_record_properties,
            max_transaction_operations,
            max_model_count,
            max_cached_items_per_model,
            max_items_per_model_operation,
            max_model_tree_depth: DEFAULT_MAX_MODEL_TREE_DEPTH,
        }
    }

    /// Returns a copy of `self` with a customized maximum tree depth limit (§26).
    pub const fn with_max_tree_depth(mut self, max: usize) -> Self {
        self.max_tree_depth = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum node count limit (§26).
    pub const fn with_max_node_count(mut self, max: usize) -> Self {
        self.max_node_count = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum string length limit (§26).
    pub const fn with_max_string_length(mut self, max: usize) -> Self {
        self.max_string_length = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum value nesting depth limit (§26).
    pub const fn with_max_value_depth(mut self, max: usize) -> Self {
        self.max_value_depth = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum list elements limit (§26).
    pub const fn with_max_list_elements(mut self, max: usize) -> Self {
        self.max_list_elements = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum record properties limit (§26).
    pub const fn with_max_record_properties(mut self, max: usize) -> Self {
        self.max_record_properties = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum transaction operations limit (§26).
    pub const fn with_max_transaction_operations(mut self, max: usize) -> Self {
        self.max_transaction_operations = max;
        self
    }

    /// Returns a copy of `self` with customized model operational limits (§26).
    pub const fn with_model_limits(
        mut self,
        max_model_count: usize,
        max_cached_items_per_model: usize,
        max_items_per_model_operation: usize,
    ) -> Self {
        self.max_model_count = max_model_count;
        self.max_cached_items_per_model = max_cached_items_per_model;
        self.max_items_per_model_operation = max_items_per_model_operation;
        self
    }

    /// Returns a copy of `self` with a customized maximum model count limit (§26).
    pub const fn with_max_model_count(mut self, max: usize) -> Self {
        self.max_model_count = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum cached items per model limit (§26).
    pub const fn with_max_cached_items_per_model(mut self, max: usize) -> Self {
        self.max_cached_items_per_model = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum items per model operation limit (§26).
    pub const fn with_max_items_per_model_operation(mut self, max: usize) -> Self {
        self.max_items_per_model_operation = max;
        self
    }

    /// Returns a copy of `self` with a customized maximum hierarchical model depth (§8, §26).
    pub const fn with_max_model_tree_depth(mut self, max: usize) -> Self {
        self.max_model_tree_depth = max;
        self
    }

    /// Validates that a model mutation batch does not exceed per-operation item limits (§26).
    pub fn validate_model_items_batch(&self, count: usize) -> Result<(), StoreError> {
        if count > self.max_items_per_model_operation {
            return Err(StoreError::MaxItemsPerModelOperationExceeded {
                limit: self.max_items_per_model_operation,
                actual: count,
            });
        }
        Ok(())
    }

    /// Validates a `Value` against string length, nesting depth, and collection size limits.
    pub fn validate_value(&self, val: &Value) -> Result<(), StoreError> {
        self.validate_value_inner(val, 1)
    }

    fn validate_value_inner(&self, val: &Value, depth: usize) -> Result<(), StoreError> {
        if depth > self.max_value_depth {
            return Err(StoreError::MaxValueDepthExceeded {
                limit: self.max_value_depth,
                actual: depth,
            });
        }

        match val {
            Value::String(s) => {
                if s.len() > self.max_string_length {
                    return Err(StoreError::MaxStringLengthExceeded {
                        limit: self.max_string_length,
                        actual: s.len(),
                    });
                }
            }
            Value::List(items) => {
                if items.len() > self.max_list_elements {
                    return Err(StoreError::MaxListLengthExceeded {
                        limit: self.max_list_elements,
                        actual: items.len(),
                    });
                }
                for item in items {
                    self.validate_value_inner(item, depth + 1)?;
                }
            }
            Value::Record(rec) => {
                if rec.properties.len() > self.max_record_properties {
                    return Err(StoreError::MaxRecordPropertiesExceeded {
                        limit: self.max_record_properties,
                        actual: rec.properties.len(),
                    });
                }
                for prop in &rec.properties {
                    self.validate_value_inner(&prop.value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
