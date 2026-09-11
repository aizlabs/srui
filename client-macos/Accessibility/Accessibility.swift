//
// Accessibility.swift
// Accessibility
//
// Platform-neutral semantic inspection and trusted in-process automation.
//

import SemanticModel

/// Monotonic identity for a semantic session.
///
/// A handle may act only while its captured epoch is still current. This prevents a node identifier
/// retained from one session from targeting a different node after session replacement.
public struct SemanticInspectionEpoch: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible, ExpressibleByIntegerLiteral {
    public let rawValue: UInt64

    public init(_ rawValue: UInt64) {
        self.rawValue = rawValue
    }

    public init(integerLiteral value: UInt64) {
        self.rawValue = value
    }

    public static func < (lhs: SemanticInspectionEpoch, rhs: SemanticInspectionEpoch) -> Bool {
        lhs.rawValue < rhs.rawValue
    }

    public var description: String {
        "SemanticInspectionEpoch(\(rawValue))"
    }
}

/// One atomically captured semantic store revision and its session identity.
public struct SemanticInspectionSourceSnapshot: Equatable, Sendable {
    public let transaction: TransactionSnapshot
    public let epoch: SemanticInspectionEpoch

    public init(transaction: TransactionSnapshot, epoch: SemanticInspectionEpoch) {
        self.transaction = transaction
        self.epoch = epoch
    }
}

/// An action a trusted in-process automation consumer can request.
public enum SemanticAction: Equatable, Sendable {
    case activate
    case valueChanged(Value)
    case selectionChanged(ItemId)

    public var eventType: TypeRef {
        switch self {
        case .activate:
            return .EVENT_ACTIVATE
        case .valueChanged:
            return .EVENT_VALUE_CHANGED
        case .selectionChanged:
            return .EVENT_SELECTION_CHANGED
        }
    }
}

/// An action request retaining the semantic identity and tree revision captured with its handle.
public struct SemanticActionRequest: Equatable, Sendable {
    public let nodeID: NodeId
    public let expectedEpoch: SemanticInspectionEpoch
    public let observedRevision: Revision
    public let action: SemanticAction

    public init(
        nodeID: NodeId,
        expectedEpoch: SemanticInspectionEpoch,
        observedRevision: Revision,
        action: SemanticAction
    ) {
        self.nodeID = nodeID
        self.expectedEpoch = expectedEpoch
        self.observedRevision = observedRevision
        self.action = action
    }
}

/// Fail-closed errors produced while admitting an automation action.
public enum SemanticAutomationError: Error, Equatable, Sendable, CustomStringConvertible {
    case staleHandle(expected: SemanticInspectionEpoch, actual: SemanticInspectionEpoch)
    case nodeNotFound(NodeId)
    case nodeDisabled(NodeId)
    case unsupportedAction(nodeID: NodeId, eventType: TypeRef)
    case capabilityNotNegotiated(TypeRef)
    case sessionInactive

    public var description: String {
        switch self {
        case .staleHandle(let expected, let actual):
            return "semantic handle epoch \(expected) does not match current epoch \(actual)"
        case .nodeNotFound(let nodeID):
            return "semantic node \(nodeID) does not exist"
        case .nodeDisabled(let nodeID):
            return "semantic node \(nodeID) is disabled"
        case .unsupportedAction(let nodeID, let eventType):
            return "semantic node \(nodeID) does not support event \(eventType)"
        case .capabilityNotNegotiated(let eventType):
            return "semantic event capability \(eventType) was not negotiated"
        case .sessionInactive:
            return "semantic automation is unavailable while the session is inactive"
        }
    }
}

/// Effective common state for an immutable semantic node snapshot.
public struct SemanticNodeState: Equatable, Sendable {
    public let visibility: EnumToken?
    public let enabled: Bool
    public let readOnly: Bool
    public let busy: Bool
    public let selected: Bool
    public let validationState: EnumToken?

    public init(
        visibility: EnumToken? = nil,
        enabled: Bool = true,
        readOnly: Bool = false,
        busy: Bool = false,
        selected: Bool = false,
        validationState: EnumToken? = nil
    ) {
        self.visibility = visibility
        self.enabled = enabled
        self.readOnly = readOnly
        self.busy = busy
        self.selected = selected
        self.validationState = validationState
    }
}

/// Toolkit-independent, immutable representation of one semantic node.
public struct SemanticNodeSnapshot: Equatable, Sendable {
    public let id: NodeId
    /// The semantic node type. This is the node's automation role.
    public let role: TypeRef
    /// Optional registry role property, separate from the node type.
    public let roleHint: EnumToken?
    public let label: String?
    public let text: String?
    public let accessibleDescription: String?
    public let valueDescription: String?
    public let value: Value?
    public let parentID: NodeId?
    public let childIDs: [NodeId]
    /// Zero-based index among siblings, or among roots for a root node.
    public let index: Int
    /// One-based tree depth. Roots have depth 1.
    public let depth: Int
    public let state: SemanticNodeState
    public let actionKey: String?
    /// Passive server-supplied action names. They do not grant executable capability.
    public let actions: [String]
    /// Intrinsic standard events derived only from the canonical registry.
    public let supportedEventTypes: [TypeRef]
    /// Complete sparse semantic properties for inspection by advanced consumers.
    public let properties: [PropertyRef: Value]

    public init(
        id: NodeId,
        role: TypeRef,
        roleHint: EnumToken?,
        label: String?,
        text: String?,
        accessibleDescription: String?,
        valueDescription: String?,
        value: Value?,
        parentID: NodeId?,
        childIDs: [NodeId],
        index: Int,
        depth: Int,
        state: SemanticNodeState,
        actionKey: String?,
        actions: [String],
        supportedEventTypes: [TypeRef],
        properties: [PropertyRef: Value]
    ) {
        self.id = id
        self.role = role
        self.roleHint = roleHint
        self.label = label
        self.text = text
        self.accessibleDescription = accessibleDescription
        self.valueDescription = valueDescription
        self.value = value
        self.parentID = parentID
        self.childIDs = childIDs
        self.index = index
        self.depth = depth
        self.state = state
        self.actionKey = actionKey
        self.actions = actions
        self.supportedEventTypes = supportedEventTypes
        self.properties = properties
    }
}

/// Immutable semantic tree captured from exactly one transaction snapshot.
public struct SemanticTreeSnapshot: Equatable, Sendable {
    public let epoch: SemanticInspectionEpoch
    public let revision: Revision
    public let rootIDs: [NodeId]
    /// Nodes in deterministic root-order pre-order traversal.
    public let nodes: [SemanticNodeSnapshot]

    private let nodeIndicesByID: [NodeId: Int]

    public init(
        epoch: SemanticInspectionEpoch,
        revision: Revision,
        rootIDs: [NodeId],
        nodes: [SemanticNodeSnapshot]
    ) {
        self.epoch = epoch
        self.revision = revision
        self.rootIDs = rootIDs
        self.nodes = nodes

        var lookup: [NodeId: Int] = [:]
        lookup.reserveCapacity(nodes.count)
        for (index, node) in nodes.enumerated() {
            lookup[node.id] = index
        }
        self.nodeIndicesByID = lookup
    }

    public func node(_ id: NodeId) -> SemanticNodeSnapshot? {
        guard let index = nodeIndicesByID[id] else { return nil }
        return nodes[index]
    }

    public subscript(id: NodeId) -> SemanticNodeSnapshot? {
        node(id)
    }
}

/// Stable reference to a node from a particular semantic tree snapshot.
public struct SemanticNodeHandle: Sendable {
    public let node: SemanticNodeSnapshot
    public let expectedEpoch: SemanticInspectionEpoch
    /// Authoritative tree revision represented by this handle's immutable node snapshot.
    public let observedRevision: Revision

    private let actionHandler: @Sendable (SemanticActionRequest) async throws -> Event

    init(
        node: SemanticNodeSnapshot,
        expectedEpoch: SemanticInspectionEpoch,
        observedRevision: Revision,
        actionHandler: @escaping @Sendable (SemanticActionRequest) async throws -> Event
    ) {
        self.node = node
        self.expectedEpoch = expectedEpoch
        self.observedRevision = observedRevision
        self.actionHandler = actionHandler
    }

    public var id: NodeId { node.id }
    public var role: TypeRef { node.role }
    public var label: String? { node.label }
    public var supportedEventTypes: [TypeRef] { node.supportedEventTypes }

    @discardableResult
    public func perform(_ action: SemanticAction) async throws -> Event {
        try await actionHandler(
            SemanticActionRequest(
                nodeID: node.id,
                expectedEpoch: expectedEpoch,
                observedRevision: observedRevision,
                action: action
            )
        )
    }

    @discardableResult
    public func activate() async throws -> Event {
        try await perform(.activate)
    }

    @discardableResult
    public func setValue(_ value: Value) async throws -> Event {
        try await perform(.valueChanged(value))
    }

    @discardableResult
    public func select(_ itemID: ItemId) async throws -> Event {
        try await perform(.selectionChanged(itemID))
    }
}

/// Fresh semantic snapshots and trusted action handles without exposing native widgets.
public struct SemanticInspector: Sendable {
    public typealias SnapshotProvider =
        @Sendable () -> SemanticInspectionSourceSnapshot
    public typealias ActionHandler =
        @Sendable (SemanticActionRequest) async throws -> Event

    private let snapshotProvider: SnapshotProvider
    private let actionHandler: ActionHandler

    public init(
        snapshotProvider: @escaping SnapshotProvider,
        actionHandler: @escaping ActionHandler
    ) {
        self.snapshotProvider = snapshotProvider
        self.actionHandler = actionHandler
    }

    /// Captures and converts one fresh transaction snapshot.
    public func snapshot() -> SemanticTreeSnapshot {
        Self.makeTreeSnapshot(from: snapshotProvider())
    }

    /// Creates a handle by identifier from an existing snapshot without recapturing the tree.
    public func handle(
        for id: NodeId,
        in tree: SemanticTreeSnapshot
    ) -> SemanticNodeHandle? {
        guard let node = tree.node(id) else { return nil }
        return makeHandle(node: node, epoch: tree.epoch, revision: tree.revision)
    }

    /// Creates a handle for a node that belongs to an existing snapshot.
    ///
    /// A node from a different snapshot is rejected rather than borrowing this tree's epoch and
    /// revision.
    public func handle(
        for node: SemanticNodeSnapshot,
        in tree: SemanticTreeSnapshot
    ) -> SemanticNodeHandle? {
        guard tree.node(node.id) == node else { return nil }
        return makeHandle(node: node, epoch: tree.epoch, revision: tree.revision)
    }

    /// Finds one node by identifier in one fresh snapshot.
    public func node(id: NodeId) -> SemanticNodeHandle? {
        let tree = snapshot()
        return handle(for: id, in: tree)
    }

    /// Finds the first exact role/label match in deterministic tree order using one fresh snapshot.
    public func find(role: TypeRef, label: String? = nil) -> SemanticNodeHandle? {
        let tree = snapshot()
        guard let node = tree.nodes.first(where: {
            $0.role == role && (label == nil || $0.label == label)
        }) else {
            return nil
        }
        return makeHandle(node: node, epoch: tree.epoch, revision: tree.revision)
    }

    /// Finds all exact role/label matches in deterministic tree order using one fresh snapshot.
    public func findAll(role: TypeRef? = nil, label: String? = nil) -> [SemanticNodeHandle] {
        let tree = snapshot()
        return tree.nodes.compactMap { node in
            guard (role == nil || node.role == role),
                  (label == nil || node.label == label) else {
                return nil
            }
            return makeHandle(node: node, epoch: tree.epoch, revision: tree.revision)
        }
    }

    private func makeHandle(
        node: SemanticNodeSnapshot,
        epoch: SemanticInspectionEpoch,
        revision: Revision
    ) -> SemanticNodeHandle {
        SemanticNodeHandle(
            node: node,
            expectedEpoch: epoch,
            observedRevision: revision,
            actionHandler: actionHandler
        )
    }

    private static func makeTreeSnapshot(
        from source: SemanticInspectionSourceSnapshot
    ) -> SemanticTreeSnapshot {
        let store = source.transaction.store
        var nodes: [SemanticNodeSnapshot] = []
        nodes.reserveCapacity(store.nodeCount)
        var seen = Set<NodeId>()

        func appendNode(_ nodeID: NodeId, index: Int, depth: Int) {
            guard seen.insert(nodeID).inserted,
                  let node = store.node(for: nodeID) else {
                return
            }

            let properties = node.properties
            let state = SemanticNodeState(
                visibility: properties[.visibility]?.asEnumToken ?? .visibilityVisible,
                enabled: properties[.enabled]?.asBool ?? true,
                readOnly: properties[.readOnly]?.asBool ?? false,
                busy: properties[.busy]?.asBool ?? false,
                selected: properties[.selected]?.asBool ?? false,
                validationState: properties[.validationState]?.asEnumToken
            )
            let actions: [String]
            if case .list(let actionValues)? = properties[.actions] {
                actions = actionValues.compactMap { $0.asString }
            } else {
                actions = []
            }

            nodes.append(
                SemanticNodeSnapshot(
                    id: node.id,
                    role: node.nodeType,
                    roleHint: properties[.role]?.asEnumToken,
                    label: properties[.label]?.asString,
                    text: properties[.text]?.asString,
                    accessibleDescription: properties[.accessibleDescription]?.asString,
                    valueDescription: properties[.valueDescription]?.asString,
                    value: properties[.value],
                    parentID: node.parentID,
                    childIDs: node.orderedChildren,
                    index: index,
                    depth: depth,
                    state: state,
                    actionKey: properties[.actionKey]?.asString,
                    actions: actions,
                    supportedEventTypes: standardEventsEmitted(by: node.nodeType),
                    properties: properties
                )
            )

            for (childIndex, childID) in node.orderedChildren.enumerated() {
                appendNode(childID, index: childIndex, depth: depth + 1)
            }
        }

        for (rootIndex, rootID) in store.rootIDs.enumerated() {
            appendNode(rootID, index: rootIndex, depth: 1)
        }

        return SemanticTreeSnapshot(
            epoch: source.epoch,
            revision: source.transaction.revision,
            rootIDs: store.rootIDs,
            nodes: nodes
        )
    }
}
