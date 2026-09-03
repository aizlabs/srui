//
// SemanticStore.swift
// SemanticModel
//
// Non-authoritative in-memory semantic graph replica store (§6.2, §6.3, §13, §26).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//
// Architectural Invariants (§4):
// - Invariant 2: The client retains a non-authoritative replica of committed semantic UI state
//   plus local presentation state. The replica exists only to render, interact, inspect, cache,
//   and resume efficiently; it never becomes application truth.
// - Invariant 3: Presentation-only state (hover, pressed visuals, caret, IME composition,
//   scroll momentum, focus rings, local animation, window chrome) is local by default and
//   MUST NOT require server round trips.
// - Invariant 11: A client can render the first valid committed subtree before the complete
//   UI has arrived.
//

import Foundation

// MARK: - Semantic Node (§6.2)

/// A node within the client-side semantic graph replica (§6.2, §6.3).
///
/// In accordance with §6.3, `Node` represents replicated semantic state and exposes
/// its structural and property state as read-only to external consumers.
public struct Node: Equatable, Sendable, CustomStringConvertible {
    /// Unique identifier for this node within the session (§6.2).
    public let id: NodeId

    /// Type reference of this node in the registry (§6.4, §7.2).
    public let nodeType: TypeRef

    /// Parent node ID in the hierarchy (`nil` for root nodes).
    public internal(set) var parentID: NodeId?

    /// Strictly ordered sequence of child node IDs (§6.2).
    public internal(set) var orderedChildren: [NodeId]

    /// Sparse map of defined property values (§7.4, §7.6).
    public internal(set) var properties: [PropertyRef: Value]

    /// Constructs a new node with the specified identity, type, parent, children, and properties dictionary.
    public init(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        orderedChildren: [NodeId] = [],
        properties: [PropertyRef: Value] = [:]
    ) {
        self.id = id
        self.nodeType = nodeType
        self.parentID = parentID
        self.orderedChildren = orderedChildren
        self.properties = properties
    }

    /// Constructs a new node with an array of property pairs.
    public init(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        orderedChildren: [NodeId] = [],
        properties: [(PropertyRef, Value)]
    ) {
        self.id = id
        self.nodeType = nodeType
        self.parentID = parentID
        self.orderedChildren = orderedChildren
        var propDict: [PropertyRef: Value] = [:]
        propDict.reserveCapacity(properties.count)
        for (k, v) in properties {
            propDict[k] = v
        }
        self.properties = propDict
    }

    /// Constructs a new node with an array of `Property` structs.
    public init(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        orderedChildren: [NodeId] = [],
        properties: [Property]
    ) {
        self.id = id
        self.nodeType = nodeType
        self.parentID = parentID
        self.orderedChildren = orderedChildren
        var propDict: [PropertyRef: Value] = [:]
        propDict.reserveCapacity(properties.count)
        for p in properties {
            propDict[p.property] = p.value
        }
        self.properties = propDict
    }

    /// Returns the property value if defined on this node.
    public func getProperty(_ prop: PropertyRef) -> Value? {
        properties[prop]
    }

    /// Returns `true` if the node has a defined value for the specified property.
    public func hasProperty(_ prop: PropertyRef) -> Bool {
        properties[prop] != nil
    }

    /// Returns an iterator over all defined properties on this node.
    public var propertyEntries: [(PropertyRef, Value)] {
        Array(properties)
    }

    /// Returns the referenced `ModelId` if this node has a `model_ref` property defined (§8).
    public var modelRef: ModelId? {
        guard let val = properties[PropertyRef.modelRef] ?? properties[PropertyRef.MODEL_REF] else {
            return nil
        }
        switch val {
        case .unsignedInt(let u):
            return ModelId(u)
        case .signedInt(let i) where i >= 0:
            return ModelId(UInt64(i))
        default:
            return nil
        }
    }

    public var description: String {
        "Node(id: \(id), type: \(nodeType), parent: \(String(describing: parentID)), children: \(orderedChildren.count), properties: \(properties.count))"
    }
}

// MARK: - Store Limits (§26)

/// Configurable mandatory runtime safety limits for `SemanticStore` (§26).
public struct StoreLimits: Equatable, Sendable {
    /// Maximum allowed depth of any node in the hierarchy (root is depth 1). Default: 64.
    public var maxTreeDepth: Int

    /// Maximum total active nodes allowed in the store. Default: 100,000.
    public var maxNodeCount: Int

    /// Maximum allowed length of a UTF-8 string property in bytes. Default: 1,048,576 (1 MiB).
    public var maxStringLength: Int

    /// Maximum allowed recursion depth for nested values (lists, records). Default: 16.
    public var maxValueDepth: Int

    /// Maximum allowed element count in a single `Value.list`. Default: 10,000.
    public var maxListElements: Int

    /// Maximum allowed property count in a single `SmallRecord`. Default: 1,000.
    public var maxRecordProperties: Int

    /// Maximum allowed mutation operations in a single transaction. Default: 10,000.
    public var maxTransactionOperations: Int

    /// Maximum allowed active models in the store (§26). Default: 1,000.
    public var maxModelCount: Int

    /// Maximum allowed cached items in a single collection model (§26). Default: 100,000.
    public var maxCachedItemsPerModel: Int

    /// Maximum allowed items in a single model mutation batch (§26). Default: 10,000.
    public var maxItemsPerModelOperation: Int

    /// Constructs a `StoreLimits` instance with default values (§26).
    public init(
        maxTreeDepth: Int = 64,
        maxNodeCount: Int = 100_000,
        maxStringLength: Int = 1024 * 1024,
        maxValueDepth: Int = 16,
        maxListElements: Int = 10_000,
        maxRecordProperties: Int = 1_000,
        maxTransactionOperations: Int = 10_000,
        maxModelCount: Int = 1_000,
        maxCachedItemsPerModel: Int = 100_000,
        maxItemsPerModelOperation: Int = 10_000
    ) {
        self.maxTreeDepth = maxTreeDepth
        self.maxNodeCount = maxNodeCount
        self.maxStringLength = maxStringLength
        self.maxValueDepth = maxValueDepth
        self.maxListElements = maxListElements
        self.maxRecordProperties = maxRecordProperties
        self.maxTransactionOperations = maxTransactionOperations
        self.maxModelCount = maxModelCount
        self.maxCachedItemsPerModel = maxCachedItemsPerModel
        self.maxItemsPerModelOperation = maxItemsPerModelOperation
    }

    /// Convenience constructor with basic tree and string limits.
    public init(maxTreeDepth: Int, maxNodeCount: Int, maxStringLength: Int) {
        self.init(
            maxTreeDepth: maxTreeDepth,
            maxNodeCount: maxNodeCount,
            maxStringLength: maxStringLength,
            maxValueDepth: 16,
            maxListElements: 10_000,
            maxRecordProperties: 1_000
        )
    }

    /// Convenience constructor with basic tree and nested value limits.
    public init(
        maxTreeDepth: Int,
        maxNodeCount: Int,
        maxStringLength: Int,
        maxValueDepth: Int,
        maxListElements: Int,
        maxRecordProperties: Int
    ) {
        self.init(
            maxTreeDepth: maxTreeDepth,
            maxNodeCount: maxNodeCount,
            maxStringLength: maxStringLength,
            maxValueDepth: maxValueDepth,
            maxListElements: maxListElements,
            maxRecordProperties: maxRecordProperties,
            maxTransactionOperations: 10_000,
            maxModelCount: 1_000,
            maxCachedItemsPerModel: 100_000,
            maxItemsPerModelOperation: 10_000
        )
    }

    // MARK: - Fluent Limit Builders

    public func withMaxTreeDepth(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxTreeDepth = max
        return copy
    }

    public func withMaxNodeCount(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxNodeCount = max
        return copy
    }

    public func withMaxStringLength(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxStringLength = max
        return copy
    }

    public func withMaxValueDepth(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxValueDepth = max
        return copy
    }

    public func withMaxListElements(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxListElements = max
        return copy
    }

    public func withMaxRecordProperties(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxRecordProperties = max
        return copy
    }

    public func withMaxTransactionOperations(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxTransactionOperations = max
        return copy
    }

    public func withMaxModelCount(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxModelCount = max
        return copy
    }

    public func withMaxCachedItemsPerModel(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxCachedItemsPerModel = max
        return copy
    }

    public func withMaxItemsPerModelOperation(_ max: Int) -> StoreLimits {
        var copy = self
        copy.maxItemsPerModelOperation = max
        return copy
    }

    /// Validates a `Value` against string length, nesting depth, and collection size limits.
    public func validate(value: Value) throws {
        try validateInner(value: value, depth: 1)
    }

    private func validateInner(value: Value, depth: Int) throws {
        if depth > maxValueDepth {
            throw StoreError.maxValueDepthExceeded(limit: maxValueDepth, actual: depth)
        }

        switch value {
        case .string(let s):
            if s.utf8.count > maxStringLength {
                throw StoreError.maxStringLengthExceeded(limit: maxStringLength, actual: s.utf8.count)
            }
        case .list(let items):
            if items.count > maxListElements {
                throw StoreError.maxListLengthExceeded(limit: maxListElements, actual: items.count)
            }
            for item in items {
                try validateInner(value: item, depth: depth + 1)
            }
        case .record(let rec):
            if rec.properties.count > maxRecordProperties {
                throw StoreError.maxRecordPropertiesExceeded(limit: maxRecordProperties, actual: rec.properties.count)
            }
            for prop in rec.properties {
                try validateInner(value: prop.value, depth: depth + 1)
            }
        default:
            break
        }
    }
}

// MARK: - Store Errors (§26, §32)

/// Errors returned by `SemanticStore` mutation operations (§26).
public enum StoreError: Error, Equatable, Sendable, CustomStringConvertible {
    /// Attempted to create a node using a `NodeId` that was already used in this session (§6.2).
    case nodeIdAlreadyUsed(NodeId)
    /// The specified node was not found in the store.
    case nodeNotFound(NodeId)
    /// The specified parent node was not found in the store.
    case parentNotFound(NodeId)
    /// Reordering children received an invalid list of child IDs (must be an exact permutation).
    case invalidChildrenReorder(parentID: NodeId, reason: String)
    /// Moving a node would create a parent-child cycle in the graph.
    case cycleDetected(nodeID: NodeId, targetParent: NodeId)
    /// Operation exceeds the configured maximum tree depth limit (§26).
    case maxTreeDepthExceeded(limit: Int, actual: Int)
    /// Operation exceeds the configured maximum node count limit (§26).
    case maxNodeCountExceeded(limit: Int, current: Int)
    /// Operation exceeds the configured maximum string length limit (§26).
    case maxStringLengthExceeded(limit: Int, actual: Int)
    /// Operation exceeds the configured maximum value nesting depth (§26).
    case maxValueDepthExceeded(limit: Int, actual: Int)
    /// Value list exceeds the configured maximum element count (§26).
    case maxListLengthExceeded(limit: Int, actual: Int)
    /// Small record exceeds the configured maximum properties count (§26).
    case maxRecordPropertiesExceeded(limit: Int, actual: Int)
    /// Operation exceeds the configured maximum model count limit (§26).
    case maxModelCountExceeded(limit: Int, current: Int)
    /// Collection model exceeds the configured maximum cached items limit (§26).
    case maxCachedItemsPerModelExceeded(limit: Int, current: Int, attempted: Int)
    /// Model mutation operation exceeds the configured maximum items limit (§26).
    case maxItemsPerModelOperationExceeded(limit: Int, actual: Int)
    /// Batch exceeds the configured maximum mutation operations limit (§26).
    case maxTransactionOperationsExceeded(limit: Int, actual: Int)
    /// Invalid model delete parameters (e.g. combined identity and range selectors, §8, §13).
    case invalidModelDelete(String)
    /// The specified child insertion index is out of bounds for the parent's current children list.
    case childIndexOutOfBounds(index: Int, count: Int)
    /// Attempted to create a model using a `ModelId` that was already used in this session (§6.2, §8).
    case modelIdAlreadyUsed(ModelId)
    /// The specified model was not found in the store.
    case modelNotFound(ModelId)
    /// The specified item was not found in the collection model cache.
    case itemNotFound(ItemId)
    /// Operation index is out of bounds for the collection model.
    case modelIndexOutOfBounds(index: UInt64, count: UInt64)
    /// Attempted to insert a duplicate ItemId into the collection model.
    case duplicateItemId(modelID: ModelId, itemID: ItemId)
    /// Generic operation error when applying an operation.
    case operationError(String)

    public var description: String {
        switch self {
        case .nodeIdAlreadyUsed(let id):
            return "NodeId \(id) has already been used in this session and cannot be reused (§6.2)"
        case .nodeNotFound(let id):
            return "node \(id) not found in store"
        case .parentNotFound(let id):
            return "parent node \(id) not found in store"
        case .invalidChildrenReorder(let parentID, let reason):
            return "invalid child permutation for parent \(parentID): \(reason)"
        case .cycleDetected(let nodeID, let targetParent):
            return "cycle detected: moving node \(nodeID) under \(targetParent) creates an ancestor loop"
        case .maxTreeDepthExceeded(let limit, let actual):
            return "tree depth limit exceeded: max allowed is \(limit), attempted depth is \(actual)"
        case .maxNodeCountExceeded(let limit, let current):
            return "node count limit exceeded: max allowed is \(limit), current count is \(current)"
        case .maxStringLengthExceeded(let limit, let actual):
            return "string length limit exceeded: max allowed is \(limit) bytes, actual length is \(actual)"
        case .maxValueDepthExceeded(let limit, let actual):
            return "value nesting depth limit exceeded: max allowed is \(limit), actual depth is \(actual)"
        case .maxListLengthExceeded(let limit, let actual):
            return "list length limit exceeded: max allowed is \(limit) elements, actual length is \(actual)"
        case .maxRecordPropertiesExceeded(let limit, let actual):
            return "record properties limit exceeded: max allowed is \(limit), actual count is \(actual)"
        case .maxModelCountExceeded(let limit, let current):
            return "model count limit exceeded: max allowed is \(limit), current count is \(current)"
        case .maxCachedItemsPerModelExceeded(let limit, let current, let attempted):
            return "cached items per model limit exceeded: max allowed is \(limit), current cached is \(current), attempted is \(attempted)"
        case .maxItemsPerModelOperationExceeded(let limit, let actual):
            return "items per model operation limit exceeded: max allowed is \(limit), actual count is \(actual)"
        case .maxTransactionOperationsExceeded(let limit, let actual):
            return "transaction operations limit exceeded: max allowed is \(limit), actual count is \(actual)"
        case .invalidModelDelete(let reason):
            return "invalid model delete: \(reason)"
        case .childIndexOutOfBounds(let index, let count):
            return "child index \(index) out of bounds (current child count: \(count))"
        case .modelIdAlreadyUsed(let id):
            return "ModelId \(id) has already been used in this session and cannot be reused (§6.2)"
        case .modelNotFound(let id):
            return "model \(id) not found in store"
        case .itemNotFound(let id):
            return "item \(id) not found in model"
        case .modelIndexOutOfBounds(let index, let count):
            return "model index \(index) out of bounds (item count: \(count))"
        case .duplicateItemId(let modelID, let itemID):
            return "duplicate item ID \(itemID) in model \(modelID)"
        case .operationError(let msg):
            return "operation application error: \(msg)"
        }
    }

    /// Returns the canonical conformance error code for this store error (§32).
    public var conformanceCode: String {
        switch self {
        case .nodeIdAlreadyUsed: return "node_id_already_used"
        case .nodeNotFound: return "node_not_found"
        case .parentNotFound: return "parent_not_found"
        case .invalidChildrenReorder: return "invalid_children_reorder"
        case .cycleDetected: return "cycle_detected"
        case .maxTreeDepthExceeded: return "max_tree_depth_exceeded"
        case .maxNodeCountExceeded: return "max_node_count_exceeded"
        case .maxStringLengthExceeded: return "max_string_length_exceeded"
        case .maxValueDepthExceeded: return "max_value_depth_exceeded"
        case .maxListLengthExceeded: return "max_list_length_exceeded"
        case .maxRecordPropertiesExceeded: return "max_record_properties_exceeded"
        case .maxModelCountExceeded: return "max_model_count_exceeded"
        case .maxCachedItemsPerModelExceeded: return "max_cached_items_per_model_exceeded"
        case .maxItemsPerModelOperationExceeded: return "max_items_per_model_operation_exceeded"
        case .maxTransactionOperationsExceeded: return "max_operations_exceeded"
        case .invalidModelDelete: return "invalid_model_delete"
        case .childIndexOutOfBounds: return "child_index_out_of_bounds"
        case .modelIdAlreadyUsed: return "model_id_already_used"
        case .modelNotFound: return "model_not_found"
        case .itemNotFound: return "item_not_found"
        case .modelIndexOutOfBounds: return "model_index_out_of_bounds"
        case .duplicateItemId: return "duplicate_item_id"
        case .operationError: return "operation_error"
        }
    }
}

// MARK: - Store Mutation Operations (§13)

/// High-level semantic mutation operation wrapping store primitives (§13).
public enum StoreOperation: Equatable, Sendable {
    /// Creates a new node in the graph (§13 CREATE_NODE, §6.2, §26).
    case createNode(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [Property] = []
    )

    /// Deletes a node and all descendants recursively (§13 DELETE_NODE, §6.2).
    case deleteNode(id: NodeId)

    /// Sets or updates a property on a node (§13 SET_PROPERTY, §26).
    case setProperty(id: NodeId, property: PropertyRef, value: Value)

    /// Clears a property from a node (§13 CLEAR_PROPERTY).
    case clearProperty(id: NodeId, property: PropertyRef)

    /// Moves a node to a new parent or index (§13 MOVE_NODE, §26).
    case moveNode(id: NodeId, newParentID: NodeId?, newChildIndex: Int?)

    /// Reorders the children of a parent node (§13 REORDER_CHILDREN).
    case reorderChildren(parentID: NodeId, newOrder: [NodeId])

    /// Sets multiple properties on a node atomically (§13 BATCH_PROPERTY_SET, §26).
    case batchPropertySet(id: NodeId, properties: [Property])

    /// Creates a new collection model (§13 CREATE_MODEL, §8).
    case createModel(id: ModelId, modelType: TypeRef, itemCount: UInt64)

    /// Inserts items into a collection model at a specified index (§13 MODEL_INSERT, §8).
    case modelInsert(id: ModelId, index: UInt64, items: [ModelItem])

    /// Deletes items from a collection model by item identity or index range (§13 MODEL_DELETE, §8).
    case modelDelete(id: ModelId, index: UInt64?, count: UInt64?, itemIds: [ItemId])

    /// Updates existing items in a collection model (§13 MODEL_UPDATE, §8).
    case modelUpdate(id: ModelId, index: UInt64?, items: [ModelItem])

    /// Resets/replaces a range of cached items in a collection model (§13 MODEL_RESET_RANGE, §8).
    case modelResetRange(id: ModelId, startIndex: UInt64, items: [ModelItem], totalCount: UInt64?)

    /// Convenience factory for creating a `createNode` operation with property pairs.
    public static func create(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [(PropertyRef, Value)] = []
    ) -> StoreOperation {
        .createNode(
            id: id,
            nodeType: nodeType,
            parentID: parentID,
            childIndex: childIndex,
            properties: properties.map { Property(property: $0.0, value: $0.1) }
        )
    }

    /// Convenience factory for creating a `createNode` operation with property pairs.
    public static func createNode(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [(PropertyRef, Value)]
    ) -> StoreOperation {
        .createNode(
            id: id,
            nodeType: nodeType,
            parentID: parentID,
            childIndex: childIndex,
            properties: properties.map { Property(property: $0.0, value: $0.1) }
        )
    }

    /// Convenience factory for creating a `batchPropertySet` operation with property pairs.
    public static func batch(
        id: NodeId,
        properties: [(PropertyRef, Value)]
    ) -> StoreOperation {
        .batchPropertySet(
            id: id,
            properties: properties.map { Property(property: $0.0, value: $0.1) }
        )
    }

    /// Convenience factory for creating a `batchPropertySet` operation with property pairs.
    public static func batchPropertySet(
        id: NodeId,
        properties: [(PropertyRef, Value)]
    ) -> StoreOperation {
        .batchPropertySet(
            id: id,
            properties: properties.map { Property(property: $0.0, value: $0.1) }
        )
    }

    /// Convenience factory for creating a `modelDelete` operation by item IDs.
    public static func modelDeleteItems(id: ModelId, itemIds: [ItemId]) -> StoreOperation {
        .modelDelete(id: id, index: nil, count: nil, itemIds: itemIds)
    }

    /// Convenience factory for creating a `modelDelete` operation by index range.
    public static func modelDeleteRange(id: ModelId, index: UInt64, count: UInt64) -> StoreOperation {
        .modelDelete(id: id, index: index, count: count, itemIds: [])
    }

    /// Applies this operation to a mutable `SemanticStore` instance (§13).
    public func apply(to store: inout SemanticStore) throws {
        switch self {
        case .createNode(let id, let nodeType, let parentID, let childIndex, let properties):
            try store.createNode(id: id, nodeType: nodeType, parentID: parentID, childIndex: childIndex, properties: properties)
        case .deleteNode(let id):
            _ = try store.deleteNode(id)
        case .setProperty(let id, let property, let value):
            _ = try store.setProperty(nodeID: id, property: property, value: value)
        case .clearProperty(let id, let property):
            _ = try store.clearProperty(nodeID: id, property: property)
        case .moveNode(let id, let newParentID, let newChildIndex):
            try store.moveNode(nodeID: id, newParentID: newParentID, newChildIndex: newChildIndex)
        case .reorderChildren(let parentID, let newOrder):
            try store.reorderChildren(parentID: parentID, newOrder: newOrder)
        case .batchPropertySet(let id, let properties):
            try store.batchPropertySet(nodeID: id, properties: properties)
        case .createModel(let id, let modelType, let itemCount):
            try store.createModel(id: id, modelType: modelType, itemCount: itemCount)
        case .modelInsert(let id, let index, let items):
            try store.modelInsert(id: id, index: index, items: items)
        case .modelDelete(let id, let index, let count, let itemIds):
            try store.modelDelete(id: id, index: index, count: count, itemIds: itemIds)
        case .modelUpdate(let id, let index, let items):
            try store.modelUpdate(id: id, index: index, items: items)
        case .modelResetRange(let id, let startIndex, let items, let totalCount):
            try store.modelResetRange(id: id, startIndex: startIndex, items: items, totalCount: totalCount)
        }
    }

    /// Returns `true` if this operation is a scalar `setProperty` mutation (§7.6, §20.2).
    public var isScalarSetProperty: Bool {
        if case .setProperty(_, _, let value) = self {
            return value.isScalar
        }
        return false
    }
}

/// Convenience alias matching protocol terminology (§13).
public typealias Operation = StoreOperation

// MARK: - SemanticStore (§6.2, §6.3, §13, §26)

/// Non-authoritative client replica of the semantic node graph (§6.2, §6.3, §12, §26).
///
/// # State Ownership & Replica Role (§6.3, Invariant 2)
///
/// Unlike the authoritative server-side store which computes application logic, the client-side
/// `SemanticStore` is an explicitly **non-authoritative replica**. It exists solely to:
/// 1. Maintain a local reflected graph of committed semantic UI state;
/// 2. Supply semantic hierarchy and property state to the local platform renderer (AppKit, §22);
/// 3. Provide an inspection and accessibility interface (§22.8, §22.9);
/// 4. Enforce strict structural invariants (ID uniqueness, parent existence, depth/count limits)
///    so that malformed or malicious wire streams cannot corrupt client state (§26).
///
/// Presentation-only state (hover, caret, scroll momentum, window positions) is intentionally
/// local and separate from this store (Invariant 3).
public struct SemanticStore: Equatable, Sendable {
    /// Active nodes mapped by `NodeId`.
    private var nodes: [NodeId: Node]

    /// Top-level root node IDs (`parent_id == nil`), in insertion order.
    private var roots: [NodeId]

    /// Set of all `NodeId`s that have ever been created in this session (§6.2 invariant).
    private var usedIDs: Set<NodeId>

    /// Active collection models mapped by `ModelId` (§8, §13).
    private var models: [ModelId: Model]

    /// Set of all `ModelId`s that have ever been created in this session (§6.2, §8 invariant).
    private var usedModelIDs: Set<ModelId>

    /// Mandatory runtime limits enforced by the store (§26).
    private let limitsValue: StoreLimits

    /// Authoritative committed revision counter (§12.1).
    private var revisionValue: Revision

    // MARK: - Initializers

    /// Constructs a new empty `SemanticStore` with default limits and baseline revision 0 (§12.1, §26).
    public init() {
        self.init(limits: StoreLimits(), revision: .initial)
    }

    /// Constructs a new empty `SemanticStore` with custom configured limits and baseline revision 0 (§12.1, §26).
    public init(limits: StoreLimits) {
        self.init(limits: limits, revision: .initial)
    }

    /// Constructs a new `SemanticStore` with configured limits and initial committed revision (§12.1, §18).
    public init(limits: StoreLimits, revision: Revision) {
        self.nodes = [:]
        self.roots = []
        self.usedIDs = []
        self.models = [:]
        self.usedModelIDs = []
        self.limitsValue = limits
        self.revisionValue = revision
    }

    /// Internal constructor for cloning/staging.
    internal init(
        nodes: [NodeId: Node],
        roots: [NodeId],
        usedIDs: Set<NodeId>,
        models: [ModelId: Model],
        usedModelIDs: Set<ModelId>,
        limits: StoreLimits,
        revision: Revision
    ) {
        self.nodes = nodes
        self.roots = roots
        self.usedIDs = usedIDs
        self.models = models
        self.usedModelIDs = usedModelIDs
        self.limitsValue = limits
        self.revisionValue = revision
    }

    // MARK: - Read-only Graph Inspection (§6.2, §6.3, §22.9)

    /// Returns the store's configured runtime safety limits (§26).
    public var limits: StoreLimits {
        limitsValue
    }

    /// Returns the store's current committed revision (§12.1).
    public var revision: Revision {
        revisionValue
    }

    /// Returns the number of active nodes currently in the store.
    public var nodeCount: Int {
        nodes.count
    }

    /// Returns `true` if the store contains no active nodes.
    public var isEmpty: Bool {
        nodes.isEmpty
    }

    /// Returns `true` if an active node exists with the given ID.
    public func containsNode(_ id: NodeId) -> Bool {
        nodes[id] != nil
    }

    /// Returns `true` if the given `NodeId` was ever used in this session (even if deleted, §6.2).
    public func isIDUsed(_ id: NodeId) -> Bool {
        usedIDs.contains(id)
    }

    /// Returns the node with the given ID, if active in the store.
    public func getNode(_ id: NodeId) -> Node? {
        nodes[id]
    }

    /// Returns the node with the given ID, if active in the store.
    public func node(for id: NodeId) -> Node? {
        nodes[id]
    }

    /// Collects every resource hash referenced by node properties or cached model items (§14).
    public func referencedResourceHashes() -> Set<ResourceHash> {
        var hashes = Set<ResourceHash>()
        for node in nodes.values {
            for (_, value) in node.properties {
                value.collectResourceHashes(into: &hashes)
            }
        }
        for model in models.values {
            for (_, item) in model.iterCachedItems() {
                item.value.collectResourceHashes(into: &hashes)
                for (_, value) in item.properties {
                    value.collectResourceHashes(into: &hashes)
                }
            }
        }
        return hashes
    }

    /// Returns a slice of the top-level root node IDs in insertion order.
    public var rootIDs: [NodeId] {
        roots
    }

    /// Returns the ordered child IDs for the given parent node, or `nil` if the parent does not exist.
    public func children(of parentID: NodeId) -> [NodeId]? {
        nodes[parentID]?.orderedChildren
    }

    /// Returns the parent ID of the given node, or `nil` if the node does not exist (nested `nil` for root nodes).
    public func parent(of id: NodeId) -> NodeId?? {
        nodes[id].map { $0.parentID }
    }

    /// Calculates the depth of a node in the hierarchy (root is depth 1).
    public func nodeDepth(_ id: NodeId) -> Int? {
        var currentID = id
        var depth = 0
        while true {
            guard let node = nodes[currentID] else { return nil }
            depth += 1
            if let parent = node.parentID {
                currentID = parent
            } else {
                break
            }
        }
        return depth
    }

    /// Calculates the maximum depth of any node within the subtree rooted at `id` (relative to `id`, root of subtree is 1).
    public func subtreeDepth(_ id: NodeId) -> Int {
        var maxChildDepth = 0
        if let node = nodes[id] {
            for childID in node.orderedChildren {
                let childDepth = subtreeDepth(childID)
                if childDepth > maxChildDepth {
                    maxChildDepth = childDepth
                }
            }
        }
        return 1 + maxChildDepth
    }

    // MARK: - Staging & Atomic Batch Execution (§12.1, §13)

    /// Creates a private staging clone of the store for atomic batch application (§12.1).
    public func cloneStaging() -> SemanticStore {
        SemanticStore(
            nodes: self.nodes,
            roots: self.roots,
            usedIDs: self.usedIDs,
            models: self.models,
            usedModelIDs: self.usedModelIDs,
            limits: self.limitsValue,
            revision: self.revisionValue
        )
    }

    /// Commits the contents of a successful staging store into this store (§12.1).
    public mutating func commitStaging(_ staged: SemanticStore, newRevision: Revision? = nil) {
        self.nodes = staged.nodes
        self.roots = staged.roots
        self.usedIDs = staged.usedIDs
        self.models = staged.models
        self.usedModelIDs = staged.usedModelIDs
        if let rev = newRevision {
            self.revisionValue = rev
        }
    }

    /// Applies a single mutation operation directly to this store (§13).
    public mutating func apply(_ operation: StoreOperation) throws {
        try operation.apply(to: &self)
    }

    /// Applies a list of mutation operations atomically: if any operation fails,
    /// the store is guaranteed to remain completely unchanged in its pre-call state (§12.1).
    public mutating func apply(_ operations: [StoreOperation]) throws {
        if operations.count > limitsValue.maxTransactionOperations {
            throw StoreError.maxTransactionOperationsExceeded(
                limit: limitsValue.maxTransactionOperations,
                actual: operations.count
            )
        }
        var staged = self.cloneStaging()
        for op in operations {
            try op.apply(to: &staged)
        }
        self.commitStaging(staged)
    }

    /// Applies a sequence of mutation operations as an atomic transaction advancing from `baseRevision` to `baseRevision + 1` (§12.1).
    public mutating func applyTransaction(
        baseRevision: Revision,
        operations: [Operation]
    ) -> Result<Revision, TxnError> {
        let applier = TransactionApplier(store: self)
        let res = applier.apply(baseRevision: baseRevision, operations: operations)
        if case .success = res {
            self = applier.currentSnapshot.store
        }
        return res
    }

    /// Applies a structured `Transaction` record, validating base revision, target revision, and operational limits (§12.1).
    public mutating func applyTransactionRecord(_ record: Transaction) -> Result<Revision, TxnError> {
        let applier = TransactionApplier(store: self)
        let res = applier.apply(record: record)
        if case .success = res {
            self = applier.currentSnapshot.store
        }
        return res
    }

    // MARK: - Low-Level Mutation Primitives (§13, §26)

    /// Validates referential integrity for semantic properties (e.g. ensuring `PropertyRef.modelRef` references an existing model).
    private func validatePropertyReferences(property: PropertyRef, value: Value) throws {
        if property == .modelRef || property == .MODEL_REF {
            let modelID: ModelId
            switch value {
            case .unsignedInt(let u):
                modelID = ModelId(u)
            case .signedInt(let i) where i >= 0:
                modelID = ModelId(UInt64(i))
            default:
                throw StoreError.operationError("model_ref property must be a non-negative integer")
            }
            guard models[modelID] != nil else {
                throw StoreError.modelNotFound(modelID)
            }
        }
    }

    /// Creates a new node in the graph (§13 CREATE_NODE, §6.2, §26).
    public mutating func createNode(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [(PropertyRef, Value)] = []
    ) throws {
        // 1. §6.2 Invariant: NodeId never reused in session
        if usedIDs.contains(id) {
            throw StoreError.nodeIdAlreadyUsed(id)
        }

        // 2. §26 Limit: max_node_count
        if nodes.count >= limitsValue.maxNodeCount {
            throw StoreError.maxNodeCountExceeded(
                limit: limitsValue.maxNodeCount,
                current: nodes.count
            )
        }

        // 3. Check parent existence and calculate depth (§26 max_tree_depth)
        let depth: Int
        if let pid = parentID {
            guard let parentDepth = nodeDepth(pid) else {
                throw StoreError.parentNotFound(pid)
            }
            let targetDepth = parentDepth + 1
            if targetDepth > limitsValue.maxTreeDepth {
                throw StoreError.maxTreeDepthExceeded(
                    limit: limitsValue.maxTreeDepth,
                    actual: targetDepth
                )
            }
            depth = targetDepth
        } else {
            depth = 1
        }

        if depth > limitsValue.maxTreeDepth {
            throw StoreError.maxTreeDepthExceeded(
                limit: limitsValue.maxTreeDepth,
                actual: depth
            )
        }

        // 4. Validate all properties, limits, and referential integrity (§26)
        for (prop, val) in properties {
            try limitsValue.validate(value: val)
            try validatePropertyReferences(property: prop, value: val)
        }

        // 5. Validate insertion index bounds
        if let pid = parentID {
            guard let parentNode = nodes[pid] else {
                throw StoreError.parentNotFound(pid)
            }
            let childCount = parentNode.orderedChildren.count
            if let idx = childIndex, idx < 0 || idx > childCount {
                throw StoreError.childIndexOutOfBounds(index: idx, count: childCount)
            }
        } else {
            if let idx = childIndex, idx < 0 || idx > roots.count {
                throw StoreError.childIndexOutOfBounds(index: idx, count: roots.count)
            }
        }

        // 6. Insert into parent's orderedChildren or roots list
        if let pid = parentID {
            if let idx = childIndex {
                nodes[pid]?.orderedChildren.insert(id, at: idx)
            } else {
                nodes[pid]?.orderedChildren.append(id)
            }
        } else {
            if let idx = childIndex {
                roots.insert(id, at: idx)
            } else {
                roots.append(id)
            }
        }

        // 7. Insert node and record used ID
        let node = Node(
            id: id,
            nodeType: nodeType,
            parentID: parentID,
            orderedChildren: [],
            properties: properties
        )
        nodes[id] = node
        usedIDs.insert(id)
    }

    /// Overload for `createNode` taking an array of `Property` structs.
    public mutating func createNode(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [Property]
    ) throws {
        try createNode(
            id: id,
            nodeType: nodeType,
            parentID: parentID,
            childIndex: childIndex,
            properties: properties.map { ($0.property, $0.value) }
        )
    }

    /// Convenience overload for `createNode` taking a dictionary of properties.
    public mutating func createNode(
        id: NodeId,
        nodeType: TypeRef,
        parentID: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [PropertyRef: Value]
    ) throws {
        try createNode(
            id: id,
            nodeType: nodeType,
            parentID: parentID,
            childIndex: childIndex,
            properties: Array(properties)
        )
    }

    /// Deletes a node and all of its descendants recursively, returning all deleted node IDs (§13 DELETE_NODE, §6.2).
    @discardableResult
    public mutating func deleteNode(_ id: NodeId) throws -> [NodeId] {
        guard let node = nodes[id] else {
            throw StoreError.nodeNotFound(id)
        }

        // 1. Remove from parent's orderedChildren or roots list
        if let pid = node.parentID {
            nodes[pid]?.orderedChildren.removeAll { $0 == id }
        } else {
            roots.removeAll { $0 == id }
        }

        // 2. Recursively delete node and all descendants
        var deleted: [NodeId] = []
        deleteSubtreeRecursive(id, deleted: &deleted)
        return deleted
    }

    private mutating func deleteSubtreeRecursive(_ id: NodeId, deleted: inout [NodeId]) {
        if let node = nodes.removeValue(forKey: id) {
            deleted.append(id)
            for childID in node.orderedChildren {
                deleteSubtreeRecursive(childID, deleted: &deleted)
            }
        }
    }

    /// Sets or updates a property on a node, returning the previous value if defined (§13 SET_PROPERTY, §26).
    @discardableResult
    public mutating func setProperty(
        nodeID: NodeId,
        property: PropertyRef,
        value: Value
    ) throws -> Value? {
        try limitsValue.validate(value: value)
        try validatePropertyReferences(property: property, value: value)
        guard nodes[nodeID] != nil else {
            throw StoreError.nodeNotFound(nodeID)
        }
        let previous = nodes[nodeID]?.properties[property]
        nodes[nodeID]?.properties[property] = value
        return previous
    }

    /// Clears a property from a node (§13 CLEAR_PROPERTY).
    @discardableResult
    public mutating func clearProperty(
        nodeID: NodeId,
        property: PropertyRef
    ) throws -> Value? {
        guard nodes[nodeID] != nil else {
            throw StoreError.nodeNotFound(nodeID)
        }
        return nodes[nodeID]?.properties.removeValue(forKey: property)
    }

    /// Sets multiple properties on a node atomically (§13 BATCH_PROPERTY_SET, §26).
    public mutating func batchPropertySet(
        nodeID: NodeId,
        properties: [(PropertyRef, Value)]
    ) throws {
        for (prop, val) in properties {
            try limitsValue.validate(value: val)
            try validatePropertyReferences(property: prop, value: val)
        }
        guard nodes[nodeID] != nil else {
            throw StoreError.nodeNotFound(nodeID)
        }
        for (prop, val) in properties {
            nodes[nodeID]?.properties[prop] = val
        }
    }

    /// Overload for `batchPropertySet` taking an array of `Property` structs.
    public mutating func batchPropertySet(
        nodeID: NodeId,
        properties: [Property]
    ) throws {
        try batchPropertySet(nodeID: nodeID, properties: properties.map { ($0.property, $0.value) })
    }

    /// Convenience overload for `batchPropertySet` taking a dictionary.
    public mutating func batchPropertySet(
        nodeID: NodeId,
        properties: [PropertyRef: Value]
    ) throws {
        try batchPropertySet(nodeID: nodeID, properties: Array(properties))
    }

    /// Moves a node to a new parent and/or child index (§13 MOVE_NODE, §26).
    public mutating func moveNode(
        nodeID: NodeId,
        newParentID: NodeId?,
        newChildIndex: Int?
    ) throws {
        guard nodes[nodeID] != nil else {
            throw StoreError.nodeNotFound(nodeID)
        }

        // 1. Cycle detection: newParentID cannot be nodeID or any descendant of nodeID
        if let targetParent = newParentID {
            if targetParent == nodeID {
                throw StoreError.cycleDetected(nodeID: nodeID, targetParent: targetParent)
            }
            var curr = targetParent
            while let parentNode = nodes[curr] {
                if let pid = parentNode.parentID {
                    if pid == nodeID {
                        throw StoreError.cycleDetected(nodeID: nodeID, targetParent: targetParent)
                    }
                    curr = pid
                } else {
                    break
                }
            }
        }

        // 2. Tree depth limit validation (§26)
        let subDepth = subtreeDepth(nodeID)
        let newParentDepth: Int
        if let pid = newParentID {
            guard let pDepth = nodeDepth(pid) else {
                throw StoreError.parentNotFound(pid)
            }
            newParentDepth = pDepth
        } else {
            newParentDepth = 0
        }

        let newTotalDepth = newParentDepth + subDepth
        if newTotalDepth > limitsValue.maxTreeDepth {
            throw StoreError.maxTreeDepthExceeded(
                limit: limitsValue.maxTreeDepth,
                actual: newTotalDepth
            )
        }

        let oldParentID = nodes[nodeID]?.parentID

        // 3. Validate newChildIndex bounds against current destination container length
        let currentDestLen: Int
        if let pid = newParentID {
            guard let pNode = nodes[pid] else {
                throw StoreError.parentNotFound(pid)
            }
            currentDestLen = pNode.orderedChildren.count
        } else {
            currentDestLen = roots.count
        }

        if let idx = newChildIndex, idx < 0 || idx > currentDestLen {
            throw StoreError.childIndexOutOfBounds(index: idx, count: currentDestLen)
        }

        // 4. Remove from old location
        if let oldPID = oldParentID {
            nodes[oldPID]?.orderedChildren.removeAll { $0 == nodeID }
        } else {
            roots.removeAll { $0 == nodeID }
        }

        // 5. Insert into new location
        if let pid = newParentID {
            if let idx = newChildIndex {
                let targetIdx = min(idx, nodes[pid]?.orderedChildren.count ?? 0)
                nodes[pid]?.orderedChildren.insert(nodeID, at: targetIdx)
            } else {
                nodes[pid]?.orderedChildren.append(nodeID)
            }
        } else {
            if let idx = newChildIndex {
                let targetIdx = min(idx, roots.count)
                roots.insert(nodeID, at: targetIdx)
            } else {
                roots.append(nodeID)
            }
        }

        // 6. Update node's parentID
        nodes[nodeID]?.parentID = newParentID
    }

    /// Reorders the children of a parent node to match a given sequence (§13 REORDER_CHILDREN).
    public mutating func reorderChildren(
        parentID: NodeId,
        newOrder: [NodeId]
    ) throws {
        guard let parentNode = nodes[parentID] else {
            throw StoreError.parentNotFound(parentID)
        }
        let currentChildren = parentNode.orderedChildren

        if newOrder.count != currentChildren.count {
            throw StoreError.invalidChildrenReorder(
                parentID: parentID,
                reason: "expected \(currentChildren.count) children, got \(newOrder.count)"
            )
        }

        let currentSet = Set(currentChildren)
        var newSet = Set<NodeId>()
        newSet.reserveCapacity(newOrder.count)

        for child in newOrder {
            if !currentSet.contains(child) {
                throw StoreError.invalidChildrenReorder(
                    parentID: parentID,
                    reason: "child \(child) is not a child of parent \(parentID)"
                )
            }
            if !newSet.insert(child).inserted {
                throw StoreError.invalidChildrenReorder(
                    parentID: parentID,
                    reason: "duplicate child \(child) in permutation list"
                )
            }
        }

        nodes[parentID]?.orderedChildren = newOrder
    }

    // MARK: - Collection Models (§8, §13)

    /// Returns the number of active models currently in the store (§8).
    public var modelCount: Int {
        models.count
    }

    /// Returns the IDs of all active models in the store (§8).
    public var modelIDs: [ModelId] {
        Array(models.keys)
    }

    /// Returns `true` if an active model exists with the given ID (§8).
    public func containsModel(_ id: ModelId) -> Bool {
        models[id] != nil
    }

    /// Returns `true` if the given `ModelId` was ever used in this session (even if deleted, §6.2, §8).
    public func isModelIDUsed(_ id: ModelId) -> Bool {
        usedModelIDs.contains(id)
    }

    /// Returns the model with the given ID, if active in the store.
    public func getModel(_ id: ModelId) -> Model? {
        models[id]
    }

    /// Returns the model referenced by the given node, if any (§8).
    public func getModelForNode(_ nodeID: NodeId) -> Model? {
        guard let node = nodes[nodeID], let mId = node.modelRef else {
            return nil
        }
        return models[mId]
    }

    /// Creates a new collection model in the store (§13 CREATE_MODEL, §8).
    public mutating func createModel(
        id: ModelId,
        modelType: TypeRef,
        itemCount: UInt64
    ) throws {
        if usedModelIDs.contains(id) {
            throw StoreError.modelIdAlreadyUsed(id)
        }
        if models.count >= limitsValue.maxModelCount {
            throw StoreError.maxModelCountExceeded(
                limit: limitsValue.maxModelCount,
                current: models.count
            )
        }

        let model = Model(id: id, modelType: modelType, itemCount: itemCount)
        models[id] = model
        usedModelIDs.insert(id)
    }

    /// Deletes a model from the store, returning the deleted model if it existed.
    @discardableResult
    public mutating func deleteModel(_ id: ModelId) throws -> Model? {
        guard models[id] != nil else {
            throw StoreError.modelNotFound(id)
        }
        return models.removeValue(forKey: id)
    }

    /// Inserts items into a collection model at a specified index (§13 MODEL_INSERT).
    public mutating func modelInsert(
        id: ModelId,
        index: UInt64,
        items: [ModelItem]
    ) throws {
        if items.count > limitsValue.maxItemsPerModelOperation {
            throw StoreError.maxItemsPerModelOperationExceeded(
                limit: limitsValue.maxItemsPerModelOperation,
                actual: items.count
            )
        }

        for item in items {
            try limitsValue.validate(value: item.value)
            for (_, val) in item.properties {
                try limitsValue.validate(value: val)
            }
        }

        guard var model = models[id] else {
            throw StoreError.modelNotFound(id)
        }

        let projectedCached = model.cachedItemCount + items.count
        if projectedCached > limitsValue.maxCachedItemsPerModel {
            throw StoreError.maxCachedItemsPerModelExceeded(
                limit: limitsValue.maxCachedItemsPerModel,
                current: model.cachedItemCount,
                attempted: projectedCached
            )
        }

        try model.insertItems(index: index, items: items)
        models[id] = model
    }

    /// Deletes items from a collection model by item identity or index range (§13 MODEL_DELETE).
    public mutating func modelDelete(
        id: ModelId,
        index: UInt64?,
        count: UInt64?,
        itemIds: [ItemId]
    ) throws {
        if itemIds.count > limitsValue.maxItemsPerModelOperation {
            throw StoreError.maxItemsPerModelOperationExceeded(
                limit: limitsValue.maxItemsPerModelOperation,
                actual: itemIds.count
            )
        }

        guard var model = models[id] else {
            throw StoreError.modelNotFound(id)
        }

        try model.deleteItems(index: index, count: count, itemIds: itemIds)
        models[id] = model
    }

    /// Updates existing items in a collection model (§13 MODEL_UPDATE).
    public mutating func modelUpdate(
        id: ModelId,
        index: UInt64?,
        items: [ModelItem]
    ) throws {
        if items.count > limitsValue.maxItemsPerModelOperation {
            throw StoreError.maxItemsPerModelOperationExceeded(
                limit: limitsValue.maxItemsPerModelOperation,
                actual: items.count
            )
        }

        for item in items {
            try limitsValue.validate(value: item.value)
            for (_, val) in item.properties {
                try limitsValue.validate(value: val)
            }
        }

        guard var model = models[id] else {
            throw StoreError.modelNotFound(id)
        }

        try model.updateItems(index: index, items: items)
        models[id] = model
    }

    /// Resets/replaces a range of cached items in a collection model (§13 MODEL_RESET_RANGE).
    public mutating func modelResetRange(
        id: ModelId,
        startIndex: UInt64,
        items: [ModelItem],
        totalCount: UInt64?
    ) throws {
        if items.count > limitsValue.maxItemsPerModelOperation {
            throw StoreError.maxItemsPerModelOperationExceeded(
                limit: limitsValue.maxItemsPerModelOperation,
                actual: items.count
            )
        }

        for item in items {
            try limitsValue.validate(value: item.value)
            for (_, val) in item.properties {
                try limitsValue.validate(value: val)
            }
        }

        guard var model = models[id] else {
            throw StoreError.modelNotFound(id)
        }

        let endIndex = startIndex.addingReportingOverflow(UInt64(items.count)).partialValue
        let removedInRange = model.iterCachedItems().filter { $0.0 >= startIndex && $0.0 < endIndex }.count
        let projectedCached = model.cachedItemCount - removedInRange + items.count
        if projectedCached > limitsValue.maxCachedItemsPerModel {
            throw StoreError.maxCachedItemsPerModelExceeded(
                limit: limitsValue.maxCachedItemsPerModel,
                current: model.cachedItemCount,
                attempted: projectedCached
            )
        }

        try model.resetRange(startIndex: startIndex, items: items, totalCount: totalCount)
        models[id] = model
    }
}
