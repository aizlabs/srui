//
// Event.swift
// SemanticModel
//
// Client-originated semantic events and validation helpers (§6.1, §7.6, §7.7, §16, §18.2, §27).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//
// Architecture & Protocol Invariants (§4, §6.1, §7.6, §7.7, §18.2, §27):
// - §6.1 Core Objects: An `Event` represents a client-originated semantic user action.
// - §7.6 Standard Events: Controls emit high-level semantic events (`ACTIVATE`, `VALUE_CHANGED`,
//   `SELECTION_CHANGED`, `EXPANSION_CHANGED`, `TEXT_EDIT`, `VIEWPORT_CHANGED`) rather than raw pointer coordinates.
// - §7.7 Semantic Input Routing: Events report what happened to which semantic node, carrying
//   `eventSeq`, a retry-safe `eventId`, `observedRevision`, target `nodeId`, `eventType`, and
//   event-specific arguments.
// - §18.2 Retry Safety & Deduplication: Every event that can cause side effects contains a stable
//   `eventId` unique within session lifetime, enabling retry-safe delivery across network disconnects.
// - §27 Server Validation: The server validates every client event against the current graph
//   (verifying node existence, interactive/enabled status, and revision freshness) before dispatching to handlers.
//

import Foundation

// MARK: - Event Identifier (§7.7, §16, §18.2)

/// Globally unique event identifier for deduplication and retry-safety (§7.7, §16, §18.2).
public struct EventId: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible, ExpressibleByStringLiteral {
    public let bytes: Data

    public init(_ bytes: Data) {
        self.bytes = bytes
    }

    public init(rawBytes: [UInt8]) {
        self.bytes = Data(rawBytes)
    }

    public init(string: String) {
        self.bytes = Data(string.utf8)
    }

    public init(stringLiteral value: String) {
        self.bytes = Data(value.utf8)
    }

    public init(hex: String) {
        var clean = hex
        if clean.hasPrefix("0x") {
            clean = String(clean.dropFirst(2))
        }
        var data = Data(capacity: clean.count / 2)
        var index = clean.startIndex
        while index < clean.endIndex {
            let nextIndex = clean.index(index, offsetBy: 2, limitedBy: clean.endIndex) ?? clean.endIndex
            let byteString = clean[index..<nextIndex]
            if let byte = UInt8(byteString, radix: 16) {
                data.append(byte)
            }
            index = nextIndex
        }
        self.bytes = data
    }

    public func toHex() -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }

    public var asBytes: [UInt8] {
        Array(bytes)
    }

    public var asString: String? {
        String(data: bytes, encoding: .utf8)
    }

    public var isEmpty: Bool {
        bytes.isEmpty
    }

    public var count: Int {
        bytes.count
    }

    public var description: String {
        if let s = asString, s.allSatisfy({ !$0.isASCII || (!$0.isWhitespace && ($0.isLetter || $0.isNumber || $0 == "-" || $0 == "_")) }) {
            return "EventId(\(s))"
        }
        return "EventId(0x\(toHex()))"
    }

    public static func < (lhs: EventId, rhs: EventId) -> Bool {
        lhs.bytes.lexicographicallyPrecedes(rhs.bytes)
    }
}

// MARK: - Client Instance Identifier (§16, §18)

/// Ephemeral client instance identifier distinguishing reconnecting client attachments (§16, §18).
public struct ClientInstanceId: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible, ExpressibleByStringLiteral {
    public let bytes: Data

    public init(_ bytes: Data) {
        self.bytes = bytes
    }

    public init(rawBytes: [UInt8]) {
        self.bytes = Data(rawBytes)
    }

    public init(string: String) {
        self.bytes = Data(string.utf8)
    }

    public init(stringLiteral value: String) {
        self.bytes = Data(value.utf8)
    }

    public func toHex() -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }

    public var asBytes: [UInt8] {
        Array(bytes)
    }

    public var asString: String? {
        String(data: bytes, encoding: .utf8)
    }

    public var isEmpty: Bool {
        bytes.isEmpty
    }

    public var count: Int {
        bytes.count
    }

    public var description: String {
        if let s = asString {
            return "ClientInstanceId(\(s))"
        }
        return "ClientInstanceId(0x\(toHex()))"
    }

    public static func < (lhs: ClientInstanceId, rhs: ClientInstanceId) -> Bool {
        lhs.bytes.lexicographicallyPrecedes(rhs.bytes)
    }
}

// MARK: - Semantic Event (§6.1, §7.6, §7.7, §16)

/// Client-originated semantic event representing a user interaction (§6.1, §7.6, §7.7, §16).
public struct Event: Equatable, Sendable, CustomStringConvertible {
    /// Optional client instance identifier (§16, §18).
    public var clientInstanceId: ClientInstanceId?
    /// Monotonically increasing per-client event sequence number (§7.7, §16).
    public var eventSeq: UInt64
    /// Globally unique, retry-safe event identifier for deduplication (§7.7, §16, §18.2).
    public var eventId: EventId
    /// Authoritative tree revision observed by client when generating this event (§7.7, §16).
    public var observedRevision: Revision
    /// Target semantic node identifier in the UI graph (§6.2, §7.7).
    public var nodeId: NodeId
    /// Event type reference in a namespace registry (§6.4, §7.6, §7.7).
    public var eventType: TypeRef
    /// Event payload arguments mapped by property reference (§7.6, §7.7, §16).
    public var arguments: [PropertyRef: Value]

    /// Constructs a generic semantic event.
    public init(
        clientInstanceId: ClientInstanceId? = nil,
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId,
        eventType: TypeRef,
        arguments: [PropertyRef: Value] = [:]
    ) {
        self.clientInstanceId = clientInstanceId
        self.eventSeq = eventSeq
        self.eventId = eventId
        self.observedRevision = observedRevision
        self.nodeId = nodeId
        self.eventType = eventType
        self.arguments = arguments
    }

    /// Constructs a generic semantic event with property-value array arguments.
    public init(
        clientInstanceId: ClientInstanceId? = nil,
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId,
        eventType: TypeRef,
        arguments: [(PropertyRef, Value)]
    ) {
        self.clientInstanceId = clientInstanceId
        self.eventSeq = eventSeq
        self.eventId = eventId
        self.observedRevision = observedRevision
        self.nodeId = nodeId
        self.eventType = eventType
        var dict: [PropertyRef: Value] = [:]
        dict.reserveCapacity(arguments.count)
        for (k, v) in arguments {
            dict[k] = v
        }
        self.arguments = dict
    }

    // MARK: - Convenience Constructors (§7.6)

    /// Convenience constructor for momentary control activation (`ACTIVATE`, §7.6, §7.7).
    public static func activate(
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId
    ) -> Event {
        Event(
            eventSeq: eventSeq,
            eventId: eventId,
            observedRevision: observedRevision,
            nodeId: nodeId,
            eventType: TypeRef.EVENT_ACTIVATE
        )
    }

    /// Convenience constructor for value change events (`VALUE_CHANGED`, §7.6).
    public static func valueChanged(
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId,
        value: Value
    ) -> Event {
        Event(
            eventSeq: eventSeq,
            eventId: eventId,
            observedRevision: observedRevision,
            nodeId: nodeId,
            eventType: TypeRef.EVENT_VALUE_CHANGED,
            arguments: [PropertyRef.VALUE: value]
        )
    }

    /// Convenience constructor for selection change events (`SELECTION_CHANGED`, §7.6).
    public static func selectionChanged(
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId,
        itemId: ItemId
    ) -> Event {
        Event(
            eventSeq: eventSeq,
            eventId: eventId,
            observedRevision: observedRevision,
            nodeId: nodeId,
            eventType: TypeRef.EVENT_SELECTION_CHANGED,
            arguments: [PropertyRef.VALUE: .itemID(itemId)]
        )
    }

    /// Convenience constructor for text edit events (`TEXT_EDIT`, §7.6, §22.6).
    public static func textEdit(
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId,
        text: String
    ) -> Event {
        Event(
            eventSeq: eventSeq,
            eventId: eventId,
            observedRevision: observedRevision,
            nodeId: nodeId,
            eventType: TypeRef.EVENT_TEXT_EDIT,
            arguments: [PropertyRef.TEXT: .string(text)]
        )
    }

    /// Convenience constructor for expansion change events (`EXPANSION_CHANGED`, §7.6).
    public static func expansionChanged(
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId,
        expanded: Bool
    ) -> Event {
        Event(
            eventSeq: eventSeq,
            eventId: eventId,
            observedRevision: observedRevision,
            nodeId: nodeId,
            eventType: TypeRef.EVENT_EXPANSION_CHANGED,
            arguments: [PropertyRef.VALUE: .bool(expanded)]
        )
    }

    /// Convenience constructor for viewport change events (`VIEWPORT_CHANGED`, §7.6).
    public static func viewportChanged(
        eventSeq: UInt64,
        eventId: EventId,
        observedRevision: Revision,
        nodeId: NodeId,
        size: Size
    ) -> Event {
        Event(
            eventSeq: eventSeq,
            eventId: eventId,
            observedRevision: observedRevision,
            nodeId: nodeId,
            eventType: TypeRef.EVENT_VIEWPORT_CHANGED,
            arguments: [PropertyRef.VALUE: .size(size)]
        )
    }

    // MARK: - Fluent Helpers & Accessors

    public func withClientInstanceId(_ id: ClientInstanceId?) -> Event {
        var copy = self
        copy.clientInstanceId = id
        return copy
    }

    public func withArgument(property: PropertyRef, value: Value) -> Event {
        var copy = self
        copy.arguments[property] = value
        return copy
    }

    public func getArgument(_ prop: PropertyRef) -> Value? {
        arguments[prop]
    }

    public func hasArgument(_ prop: PropertyRef) -> Bool {
        arguments[prop] != nil
    }

    public var valueArg: Value? {
        arguments[PropertyRef.VALUE] ?? arguments[PropertyRef.value]
    }

    public var textArg: String? {
        (arguments[PropertyRef.TEXT] ?? arguments[PropertyRef.text])?.asString
    }

    public var boolArg: Bool? {
        (arguments[PropertyRef.VALUE] ?? arguments[PropertyRef.value])?.asBool
    }

    public var itemIdArg: ItemId? {
        (arguments[PropertyRef.VALUE] ?? arguments[PropertyRef.value])?.asItemID
    }

    public var standardName: String? {
        eventType.standardEventName
    }

    public var description: String {
        let typeLabel = standardName ?? "\(eventType)"
        return "Event(seq: \(eventSeq), id: \(eventId), node: \(nodeId), type: \(typeLabel), rev: \(observedRevision), args: \(arguments.count))"
    }

    // MARK: - Validation Against Store (§7.7, §27)

    public func validateNodeExists(in store: SemanticStore) -> Result<Node, EventValidationError> {
        guard let node = store.getNode(nodeId) else {
            return .failure(.nodeNotFound(nodeId))
        }
        return .success(node)
    }

    public func validateNodeInteractive(in store: SemanticStore) -> Result<Node, EventValidationError> {
        switch validateNodeExists(in: store) {
        case .failure(let err):
            return .failure(err)
        case .success(let node):
            if case .bool(false) = node.getProperty(PropertyRef.ENABLED) ?? node.getProperty(PropertyRef.enabled) {
                return .failure(.nodeDisabled(nodeId))
            }
            return .success(node)
        }
    }

    public func validateObservedRevision(currentRevision: Revision) -> Result<Void, EventValidationError> {
        if observedRevision > currentRevision {
            return .failure(.futureRevision(observed: observedRevision, current: currentRevision))
        }
        return .success(())
    }

    public func validate(against store: SemanticStore) -> Result<Node, EventValidationError> {
        switch validateObservedRevision(currentRevision: store.revision) {
        case .failure(let err):
            return .failure(err)
        case .success:
            return validateNodeInteractive(in: store)
        }
    }
}

// MARK: - Event Validation Error (§7.7, §27)

/// Errors returned when validating a client-originated event against a `SemanticStore` (§7.7, §27).
public enum EventValidationError: Error, Hashable, Equatable, Sendable, CustomStringConvertible {
    /// Target `NodeId` does not exist in the store (§7.7).
    case nodeNotFound(NodeId)
    /// Target node has `enabled = false` and cannot accept interactive events (§7.4, §27).
    case nodeDisabled(NodeId)
    /// Event references an observed revision that is in the future relative to the store (§7.7, §12.1).
    case futureRevision(observed: Revision, current: Revision)
    /// Event is missing an expected argument property.
    case missingArgument(PropertyRef)

    public var description: String {
        switch self {
        case .nodeNotFound(let id):
            return "event target node \(id) does not exist in store"
        case .nodeDisabled(let id):
            return "event target node \(id) is disabled"
        case .futureRevision(let observed, let current):
            return "event observed revision \(observed) is in the future relative to store revision \(current)"
        case .missingArgument(let prop):
            return "event is missing required argument property \(prop)"
        }
    }
}
