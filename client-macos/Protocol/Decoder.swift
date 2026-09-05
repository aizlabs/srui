//
// Decoder.swift
// Protocol
//
// Wire protocol decoder and §26 client-side safety limits enforcer (§16, §22.2, §26).
//
// Architectural Invariants (§4, §16, §22.2, §26):
// - §16 Reference Wire Encoding: Pure in-memory decode and encode functions for Protobuf
//   messages without network or socket coupling.
// - §22.2 Threading: All decoders are pure, `Sendable`, thread-safe structs/functions designed
//   to run completely off the main thread/actor prior to dispatching to `TransactionApplier`.
// - §26 Mandatory Limits: Enforces max_frame_size, max_transaction_operations, max_string_length,
//   max_value_depth, max_list_elements, max_record_properties, and max_items_per_model_operation
//   at decode time. Any violation is rejected with a typed error and fails closed without touching the store.
//

import Foundation
import SwiftProtobuf
import SemanticModel

// Disambiguate Operation alias from Foundation.Operation / NSOperation
public typealias Operation = SemanticModel.StoreOperation

// MARK: - Protocol Decode Error (§16, §26)

/// Typed errors returned during wire decoding and §26 safety limits validation (§16, §26).
public enum ProtocolDecodeError: Error, Equatable, Sendable, CustomStringConvertible {
    /// Wire frame size exceeds allowable limit (§26).
    case frameSizeLimitExceeded(limit: Int, actual: Int)
    /// Malformed varint length prefix in wire frame (§16).
    case malformedVarint
    /// Frame payload truncated or unexpected EOF (§16).
    case truncatedPayload(expected: Int, actual: Int)
    /// Protobuf parsing error (malformed fields, invalid wire types).
    case protobufDecodeError(String)
    /// Expected wire field is missing.
    case missingField(String)
    /// Transaction operations count exceeds allowable limit (§26).
    case maxOperationsExceeded(limit: Int, actual: Int)
    /// String property byte length exceeds allowable limit (§26).
    case maxStringLengthExceeded(limit: Int, actual: Int)
    /// Nested value depth exceeds allowable limit (§26).
    case maxValueDepthExceeded(limit: Int, actual: Int)
    /// List value element count exceeds allowable limit (§26).
    case maxListElementsExceeded(limit: Int, actual: Int)
    /// SmallRecord property count exceeds allowable limit (§26).
    case maxRecordPropertiesExceeded(limit: Int, actual: Int)
    /// Model operation items count exceeds allowable limit (§26).
    case maxItemsPerModelOperationExceeded(limit: Int, actual: Int)
    /// Resource hash byte length is invalid (expected 32 bytes).
    case invalidResourceHashLength(Int)
    /// Operation payload variant is invalid or unknown.
    case invalidOperation(String)
    /// General protocol decoding error.
    case custom(String)

    public var description: String {
        switch self {
        case .frameSizeLimitExceeded(let limit, let actual):
            return "Wire frame size limit exceeded: max allowed is \(limit) bytes, actual is \(actual) bytes (§26)"
        case .malformedVarint:
            return "Malformed varint length prefix in wire frame"
        case .truncatedPayload(let expected, let actual):
            return "Truncated frame payload: expected \(expected) bytes, got \(actual) bytes"
        case .protobufDecodeError(let msg):
            return "Protobuf decode error: \(msg)"
        case .missingField(let field):
            return "Missing expected protobuf wire field: \(field)"
        case .maxOperationsExceeded(let limit, let actual):
            return "Transaction operations limit exceeded: \(actual) ops exceeds max limit of \(limit) (§26)"
        case .maxStringLengthExceeded(let limit, let actual):
            return "String length limit exceeded: max allowed is \(limit) bytes, actual is \(actual) bytes (§26)"
        case .maxValueDepthExceeded(let limit, let actual):
            return "Value nesting depth limit exceeded: max allowed is \(limit), actual depth is \(actual) (§26)"
        case .maxListElementsExceeded(let limit, let actual):
            return "List length limit exceeded: max allowed is \(limit) elements, actual length is \(actual) (§26)"
        case .maxRecordPropertiesExceeded(let limit, let actual):
            return "Record properties limit exceeded: max allowed is \(limit), actual count is \(actual) (§26)"
        case .maxItemsPerModelOperationExceeded(let limit, let actual):
            return "Items per model operation limit exceeded: max allowed is \(limit), actual count is \(actual) (§26)"
        case .invalidResourceHashLength(let len):
            return "Expected 32-byte resource hash, got \(len) bytes"
        case .invalidOperation(let msg):
            return "Invalid operation: \(msg)"
        case .custom(let msg):
            return "Protocol error: \(msg)"
        }
    }

    /// Canonical error code for conformance testing and logging (§32).
    public var conformanceCode: String {
        switch self {
        case .frameSizeLimitExceeded: return "frame_size_limit_exceeded"
        case .malformedVarint: return "overlong_varint"
        case .truncatedPayload: return "truncated_frame"
        case .protobufDecodeError: return "protobuf_decode_error"
        case .missingField: return "missing_field"
        case .maxOperationsExceeded: return "max_operations_exceeded"
        case .maxStringLengthExceeded: return "max_string_length_exceeded"
        case .maxValueDepthExceeded: return "max_value_depth_exceeded"
        case .maxListElementsExceeded: return "max_list_length_exceeded"
        case .maxRecordPropertiesExceeded: return "max_record_properties_exceeded"
        case .maxItemsPerModelOperationExceeded: return "max_items_per_model_operation_exceeded"
        case .invalidResourceHashLength: return "invalid_resource_hash_length"
        case .invalidOperation: return "invalid_operation"
        case .custom: return "custom_error"
        }
    }
}

// MARK: - Protocol Decoder (§16, §22.2, §26)

/// Pure, thread-safe Protobuf wire decoder enforcing §26 safety limits (§16, §22.2, §26).
public struct ProtocolDecoder: Sendable {
    /// Configured mandatory safety limits (§26).
    public var limits: StoreLimits
    /// Maximum allowed wire frame size in bytes (§26).
    public var maxFrameSize: Int

    /// Constructs a `ProtocolDecoder` with the specified limits.
    public init(
        limits: StoreLimits = StoreLimits(),
        maxFrameSize: Int = defaultMaxFrameSize
    ) {
        self.limits = limits
        self.maxFrameSize = maxFrameSize
    }

    // MARK: - Decode Operations (§16, §26)

    /// Deserializes a [`Transaction`] from raw Protobuf bytes, enforcing §26 limits.
    public func decodeTransaction(from data: Data) throws -> Transaction {
        if data.count > maxFrameSize {
            throw ProtocolDecodeError.frameSizeLimitExceeded(limit: maxFrameSize, actual: data.count)
        }

        let wireTxn: SRUITransaction
        do {
            wireTxn = try SRUITransaction(serializedBytes: data)
        } catch {
            throw ProtocolDecodeError.protobufDecodeError(error.localizedDescription)
        }

        return try validateAndConvertTransaction(wire: wireTxn)
    }

    /// Deserializes an [`Operation`] from raw Protobuf bytes, enforcing §26 limits.
    public func decodeOperation(from data: Data) throws -> StoreOperation {
        if data.count > maxFrameSize {
            throw ProtocolDecodeError.frameSizeLimitExceeded(limit: maxFrameSize, actual: data.count)
        }

        let wireOp: SRUIOperation
        do {
            wireOp = try SRUIOperation(serializedBytes: data)
        } catch {
            throw ProtocolDecodeError.protobufDecodeError(error.localizedDescription)
        }

        return try validateAndConvertOperation(wire: wireOp)
    }

    /// Deserializes an [`Event`] from raw Protobuf bytes, enforcing §26 limits.
    public func decodeEvent(from data: Data) throws -> Event {
        if data.count > maxFrameSize {
            throw ProtocolDecodeError.frameSizeLimitExceeded(limit: maxFrameSize, actual: data.count)
        }

        let wireEvent: SRUIEvent
        do {
            wireEvent = try SRUIEvent(serializedBytes: data)
        } catch {
            throw ProtocolDecodeError.protobufDecodeError(error.localizedDescription)
        }

        return try validateAndConvertEvent(wire: wireEvent)
    }

    /// Deserializes a [`Value`] from raw Protobuf bytes, enforcing §26 limits.
    public func decodeValue(from data: Data) throws -> Value {
        if data.count > maxFrameSize {
            throw ProtocolDecodeError.frameSizeLimitExceeded(limit: maxFrameSize, actual: data.count)
        }

        let wireVal: SRUIValue
        do {
            wireVal = try SRUIValue(serializedBytes: data)
        } catch {
            throw ProtocolDecodeError.protobufDecodeError(error.localizedDescription)
        }

        return try validateAndConvertValue(wire: wireVal, depth: 1)
    }

    /// Deserializes a [`NodeRecord`] from raw Protobuf bytes, enforcing §26 limits.
    public func decodeNodeRecord(from data: Data) throws -> NodeRecord {
        if data.count > maxFrameSize {
            throw ProtocolDecodeError.frameSizeLimitExceeded(limit: maxFrameSize, actual: data.count)
        }

        let wireNode: SRUINodeRecord
        do {
            wireNode = try SRUINodeRecord(serializedBytes: data)
        } catch {
            throw ProtocolDecodeError.protobufDecodeError(error.localizedDescription)
        }

        return try validateAndConvertNodeRecord(wire: wireNode)
    }

    /// Deserializes a top-level [`SRUIMessage`] from raw Protobuf bytes.
    public func decodeMessage(from data: Data) throws -> SRUIMessage {
        if data.count > maxFrameSize {
            throw ProtocolDecodeError.frameSizeLimitExceeded(limit: maxFrameSize, actual: data.count)
        }

        do {
            return try SRUIMessage(serializedBytes: data)
        } catch {
            throw ProtocolDecodeError.protobufDecodeError(error.localizedDescription)
        }
    }

    /// Deserializes a length-delimited [`SRUIMessage`] frame from Data, enforcing `maxFrameSize`.
    public func decodeFramedMessage(from data: Data) throws -> SRUIMessage {
        try SRUIFraming.decodeFramed(SRUIMessage.self, from: data, maxFrameSize: maxFrameSize)
    }

    /// Deserializes a length-delimited [`Transaction`] message frame from Data.
    public func decodeFramedTransaction(from data: Data) throws -> Transaction {
        let msg = try decodeFramedMessage(from: data)
        guard case .transaction(let tx)? = msg.msg else {
            throw ProtocolDecodeError.missingField("SruiMessage.transaction")
        }
        return try validateAndConvertTransaction(wire: tx)
    }

    /// Deserializes a length-delimited [`Event`] message frame from Data.
    public func decodeFramedEvent(from data: Data) throws -> Event {
        let msg = try decodeFramedMessage(from: data)
        guard case .event(let ev)? = msg.msg else {
            throw ProtocolDecodeError.missingField("SruiMessage.event")
        }
        return try validateAndConvertEvent(wire: ev)
    }

    // MARK: - Validation and Conversion Helpers

    public func validateAndConvertTransaction(wire: SRUITransaction) throws -> Transaction {
        let opCount = wire.operations.count
        if opCount > limits.maxTransactionOperations {
            throw ProtocolDecodeError.maxOperationsExceeded(limit: limits.maxTransactionOperations, actual: opCount)
        }

        var ops: [StoreOperation] = []
        ops.reserveCapacity(opCount)
        for wireOp in wire.operations {
            ops.append(try validateAndConvertOperation(wire: wireOp))
        }

        return Transaction(
            baseRevision: Revision(wire.baseRevision),
            newRevision: Revision(wire.newRevision),
            operations: ops,
            priority: wire.priority
        )
    }

    public func validateAndConvertOperation(wire: SRUIOperation) throws -> StoreOperation {
        guard let op = wire.op else {
            throw ProtocolDecodeError.missingField("Operation.op")
        }

        switch op {
        case .createNode(let create):
            guard create.hasNode else {
                throw ProtocolDecodeError.missingField("CreateNodeOp.node")
            }
            let record = try validateAndConvertNodeRecord(wire: create.node)
            return .createNode(
                id: record.nodeId,
                nodeType: record.nodeType,
                parentID: record.parentId,
                childIndex: record.childIndex,
                properties: record.properties
            )

        case .deleteNode(let del):
            return .deleteNode(id: NodeId(del.nodeID))

        case .setProperty(let set):
            guard set.hasProperty else {
                throw ProtocolDecodeError.missingField("SetPropertyOp.property")
            }
            let id = NodeId(set.nodeID)
            let propRef = PropertyRef(wire: set.property)
            let val = set.hasValue ? try validateAndConvertValue(wire: set.value, depth: 1) : .null
            return .setProperty(id: id, property: propRef, value: val)

        case .clearProperty_p(let clear):
            guard clear.hasProperty else {
                throw ProtocolDecodeError.missingField("ClearPropertyOp.property")
            }
            return .clearProperty(id: NodeId(clear.nodeID), property: PropertyRef(wire: clear.property))

        case .moveNode(let move):
            let id = NodeId(move.nodeID)
            let newParentID = move.newParentID == 0 ? nil : NodeId(move.newParentID)
            let newChildIndex = move.newChildIndex == UInt32.max ? nil : Int(move.newChildIndex)
            return .moveNode(id: id, newParentID: newParentID, newChildIndex: newChildIndex)

        case .reorderChildren(let reorder):
            let parentID = NodeId(reorder.parentID)
            let newOrder = reorder.childNodeIds.map { NodeId($0) }
            return .reorderChildren(parentID: parentID, newOrder: newOrder)

        case .batchPropertySet(let batch):
            let id = NodeId(batch.nodeID)
            var props: [Property] = []
            props.reserveCapacity(batch.properties.count)
            for p in batch.properties {
                guard p.hasProperty else {
                    throw ProtocolDecodeError.missingField("Property.property")
                }
                let propRef = PropertyRef(wire: p.property)
                let val = p.hasValue ? try validateAndConvertValue(wire: p.value, depth: 1) : .null
                props.append(Property(property: propRef, value: val))
            }
            return .batchPropertySet(id: id, properties: props)

        case .createModel(let create):
            let id = ModelId(create.modelID)
            guard create.hasModelType else {
                throw ProtocolDecodeError.missingField("CreateModelOp.modelType")
            }
            let modelType = TypeRef(wire: create.modelType)
            return .createModel(id: id, modelType: modelType, itemCount: create.itemCount)

        case .modelInsert(let insert):
            let id = ModelId(insert.modelID)
            if insert.items.count > limits.maxItemsPerModelOperation {
                throw ProtocolDecodeError.maxItemsPerModelOperationExceeded(
                    limit: limits.maxItemsPerModelOperation,
                    actual: insert.items.count
                )
            }
            var items: [ModelItem] = []
            items.reserveCapacity(insert.items.count)
            for item in insert.items {
                items.append(try validateAndConvertModelItem(wire: item))
            }
            return .modelInsert(id: id, index: insert.index, items: items)

        case .modelDelete(let del):
            if del.itemIds.count > limits.maxItemsPerModelOperation {
                throw ProtocolDecodeError.maxItemsPerModelOperationExceeded(
                    limit: limits.maxItemsPerModelOperation,
                    actual: del.itemIds.count
                )
            }
            let id = ModelId(del.modelID)
            let index = del.count > 0 ? del.index : nil
            let count = del.count > 0 ? del.count : nil
            let itemIds = del.itemIds.map { ItemId($0) }
            return .modelDelete(id: id, index: index, count: count, itemIds: itemIds)

        case .modelUpdate(let update):
            let id = ModelId(update.modelID)
            if update.items.count > limits.maxItemsPerModelOperation {
                throw ProtocolDecodeError.maxItemsPerModelOperationExceeded(
                    limit: limits.maxItemsPerModelOperation,
                    actual: update.items.count
                )
            }
            let index = update.index == UInt64.max ? nil : update.index
            var items: [ModelItem] = []
            items.reserveCapacity(update.items.count)
            for item in update.items {
                items.append(try validateAndConvertModelItem(wire: item))
            }
            return .modelUpdate(id: id, index: index, items: items)

        case .modelResetRange(let reset):
            let id = ModelId(reset.modelID)
            if reset.items.count > limits.maxItemsPerModelOperation {
                throw ProtocolDecodeError.maxItemsPerModelOperationExceeded(
                    limit: limits.maxItemsPerModelOperation,
                    actual: reset.items.count
                )
            }
            let totalCount = reset.totalCount > 0 ? reset.totalCount : nil
            var items: [ModelItem] = []
            items.reserveCapacity(reset.items.count)
            for item in reset.items {
                items.append(try validateAndConvertModelItem(wire: item))
            }
            return .modelResetRange(id: id, startIndex: reset.startIndex, items: items, totalCount: totalCount)

        default:
            throw ProtocolDecodeError.invalidOperation("Unsupported or unknown operation variant in wire envelope")
        }
    }

    public func validateAndConvertNodeRecord(wire: SRUINodeRecord) throws -> NodeRecord {
        guard wire.hasType else {
            throw ProtocolDecodeError.missingField("NodeRecord.type")
        }
        let nodeId = NodeId(wire.nodeID)
        let nodeType = TypeRef(wire: wire.type)
        let parentId = wire.parentID == 0 ? nil : NodeId(wire.parentID)
        let childIndex = wire.childIndex == UInt32.max ? nil : Int(wire.childIndex)

        var props: [Property] = []
        props.reserveCapacity(wire.properties.count)
        for p in wire.properties {
            guard p.hasProperty else {
                throw ProtocolDecodeError.missingField("Property.property")
            }
            let propRef = PropertyRef(wire: p.property)
            let val = p.hasValue ? try validateAndConvertValue(wire: p.value, depth: 1) : .null
            props.append(Property(property: propRef, value: val))
        }

        return NodeRecord(
            nodeId: nodeId,
            nodeType: nodeType,
            parentId: parentId,
            childIndex: childIndex,
            properties: props
        )
    }

    public func validateAndConvertModelItem(wire: SRUIModelItem) throws -> ModelItem {
        let itemID = ItemId(wire.itemID)
        let val = wire.hasValue ? try validateAndConvertValue(wire: wire.value, depth: 1) : .null

        var props: [PropertyRef: Value] = [:]
        props.reserveCapacity(wire.properties.count)
        for p in wire.properties {
            guard p.hasProperty else {
                throw ProtocolDecodeError.missingField("Property.property")
            }
            let propRef = PropertyRef(wire: p.property)
            let propVal = p.hasValue ? try validateAndConvertValue(wire: p.value, depth: 1) : .null
            props[propRef] = propVal
        }

        return ModelItem(itemID: itemID, value: val, properties: props)
    }

    public func validateAndConvertEvent(wire: SRUIEvent) throws -> Event {
        let clientInstanceId = wire.clientInstanceID.isEmpty ? nil : ClientInstanceId(wire.clientInstanceID)
        guard wire.hasEventType else {
            throw ProtocolDecodeError.missingField("Event.eventType")
        }
        let eventType = TypeRef(wire: wire.eventType)

        var args: [PropertyRef: Value] = [:]
        args.reserveCapacity(wire.arguments.count)
        for p in wire.arguments {
            guard p.hasProperty else {
                throw ProtocolDecodeError.missingField("Property.property")
            }
            let propRef = PropertyRef(wire: p.property)
            let val = p.hasValue ? try validateAndConvertValue(wire: p.value, depth: 1) : .null
            args[propRef] = val
        }

        return Event(
            clientInstanceId: clientInstanceId,
            eventSeq: wire.eventSeq,
            eventId: EventId(wire.eventID),
            observedRevision: Revision(wire.observedRevision),
            nodeId: NodeId(wire.nodeID),
            eventType: eventType,
            arguments: args,
            editSeq: EditSeq(wire.editSeq)
        )
    }

    public func validateAndConvertValue(wire: SRUIValue, depth: Int) throws -> Value {
        if depth > limits.maxValueDepth {
            throw ProtocolDecodeError.maxValueDepthExceeded(limit: limits.maxValueDepth, actual: depth)
        }

        guard let wireVal = wire.value else {
            throw ProtocolDecodeError.missingField("Value.value")
        }

        switch wireVal {
        case .nullValue:
            return .null
        case .boolValue(let b):
            return .bool(b)
        case .intValue(let i):
            return .signedInt(i)
        case .uintValue(let u):
            return .unsignedInt(u)
        case .floatValue(let f):
            return .float64(f)
        case .stringValue(let s):
            let utf8Count = s.utf8.count
            if utf8Count > limits.maxStringLength {
                throw ProtocolDecodeError.maxStringLengthExceeded(limit: limits.maxStringLength, actual: utf8Count)
            }
            return .string(s)
        case .nodeIDValue(let id):
            return .nodeID(NodeId(id))
        case .itemIDValue(let id):
            return .itemID(ItemId(id))
        case .resourceHash(let bytes):
            guard bytes.count == 32 else {
                throw ProtocolDecodeError.invalidResourceHashLength(bytes.count)
            }
            guard let hash = try? ResourceHash(bytes: bytes) else {
                throw ProtocolDecodeError.invalidResourceHashLength(bytes.count)
            }
            return .resourceHash(hash)
        case .enumValue(let e):
            return .enumToken(EnumToken(wire: e))
        case .sizeValue(let s):
            return .size(Size(wire: s))
        case .pointValue(let p):
            return .point(Point(wire: p))
        case .rangeValue(let r):
            return .range(SemanticRange(wire: r))
        case .rectValue(let r):
            return .rect(Rect(wire: r))
        case .insetsValue(let i):
            return .edgeInsets(EdgeInsets(wire: i))
        case .listValue(let l):
            let listCount = l.values.count
            if listCount > limits.maxListElements {
                throw ProtocolDecodeError.maxListElementsExceeded(limit: limits.maxListElements, actual: listCount)
            }
            var items: [Value] = []
            items.reserveCapacity(listCount)
            for v in l.values {
                items.append(try validateAndConvertValue(wire: v, depth: depth + 1))
            }
            return .list(items)
        case .recordValue(let r):
            guard r.hasType else {
                throw ProtocolDecodeError.missingField("SmallRecord.type")
            }
            let propCount = r.properties.count
            if propCount > limits.maxRecordProperties {
                throw ProtocolDecodeError.maxRecordPropertiesExceeded(limit: limits.maxRecordProperties, actual: propCount)
            }
            let typeRef = TypeRef(wire: r.type)
            var props: [Property] = []
            props.reserveCapacity(propCount)
            for p in r.properties {
                guard p.hasProperty else {
                    throw ProtocolDecodeError.missingField("Property.property")
                }
                let propRef = PropertyRef(wire: p.property)
                let val = p.hasValue ? try validateAndConvertValue(wire: p.value, depth: depth + 1) : .null
                props.append(Property(property: propRef, value: val))
            }
            return .record(SmallRecord(typeRef: typeRef, properties: props))
        }
    }

    // MARK: - Encode Operations (§16)

    /// Serializes a [`Transaction`] to Protobuf bytes.
    public func encodeTransaction(_ txn: Transaction) throws -> Data {
        let wireTxn = txn.toWire()
        return try wireTxn.serializedData()
    }

    /// Serializes an [`Operation`] to Protobuf bytes.
    public func encodeOperation(_ op: StoreOperation) throws -> Data {
        let wireOp = op.toWire()
        return try wireOp.serializedData()
    }

    /// Serializes an [`Event`] to Protobuf bytes.
    public func encodeEvent(_ event: Event) throws -> Data {
        let wireEvent = event.toWire()
        return try wireEvent.serializedData()
    }

    /// Serializes a [`Value`] to Protobuf bytes.
    public func encodeValue(_ value: Value) throws -> Data {
        let wireValue = value.toWire()
        return try wireValue.serializedData()
    }

    /// Serializes a [`NodeRecord`] to Protobuf bytes.
    public func encodeNodeRecord(_ node: NodeRecord) throws -> Data {
        let wireNode = node.toWire()
        return try wireNode.serializedData()
    }

    /// Serializes a [`SRUIMessage`] to Protobuf bytes.
    public func encodeMessage(_ msg: SRUIMessage) throws -> Data {
        return try msg.serializedData()
    }

    /// Serializes a length-delimited [`SRUIMessage`] frame to Data.
    public func encodeFramedMessage(_ msg: SRUIMessage) throws -> Data {
        return try SRUIFraming.encodeFramed(msg, maxFrameSize: maxFrameSize)
    }

    /// Serializes a length-delimited [`Transaction`] message frame to Data.
    public func encodeFramedTransaction(_ txn: Transaction) throws -> Data {
        var msg = SRUIMessage()
        msg.transaction = txn.toWire()
        return try encodeFramedMessage(msg)
    }

    /// Serializes a length-delimited [`Event`] message frame to Data.
    public func encodeFramedEvent(_ event: Event) throws -> Data {
        var msg = SRUIMessage()
        msg.event = event.toWire()
        return try encodeFramedMessage(msg)
    }
}

// MARK: - Top-Level Pure Decode & Encode Functions (§16, §22.2)

/// Deserializes a [`Transaction`] from raw Protobuf wire bytes (§16).
public func decodeTransaction(from data: Data, limits: StoreLimits = StoreLimits(), maxFrameSize: Int = defaultMaxFrameSize) throws -> Transaction {
    let decoder = ProtocolDecoder(limits: limits, maxFrameSize: maxFrameSize)
    return try decoder.decodeTransaction(from: data)
}

/// Deserializes an [`Operation`] from raw Protobuf wire bytes (§16).
public func decodeOperation(from data: Data, limits: StoreLimits = StoreLimits(), maxFrameSize: Int = defaultMaxFrameSize) throws -> StoreOperation {
    let decoder = ProtocolDecoder(limits: limits, maxFrameSize: maxFrameSize)
    return try decoder.decodeOperation(from: data)
}

/// Deserializes an [`Event`] from raw Protobuf wire bytes (§16).
public func decodeEvent(from data: Data, limits: StoreLimits = StoreLimits(), maxFrameSize: Int = defaultMaxFrameSize) throws -> Event {
    let decoder = ProtocolDecoder(limits: limits, maxFrameSize: maxFrameSize)
    return try decoder.decodeEvent(from: data)
}

/// Deserializes a [`Value`] from raw Protobuf wire bytes (§16).
public func decodeValue(from data: Data, limits: StoreLimits = StoreLimits(), maxFrameSize: Int = defaultMaxFrameSize) throws -> Value {
    let decoder = ProtocolDecoder(limits: limits, maxFrameSize: maxFrameSize)
    return try decoder.decodeValue(from: data)
}

/// Deserializes a [`NodeRecord`] from raw Protobuf wire bytes (§16).
public func decodeNodeRecord(from data: Data, limits: StoreLimits = StoreLimits(), maxFrameSize: Int = defaultMaxFrameSize) throws -> NodeRecord {
    let decoder = ProtocolDecoder(limits: limits, maxFrameSize: maxFrameSize)
    return try decoder.decodeNodeRecord(from: data)
}

/// Deserializes a [`SRUIMessage`] from raw Protobuf wire bytes (§16).
public func decodeMessage(from data: Data, maxFrameSize: Int = defaultMaxFrameSize) throws -> SRUIMessage {
    let decoder = ProtocolDecoder(maxFrameSize: maxFrameSize)
    return try decoder.decodeMessage(from: data)
}

/// Deserializes a length-delimited [`SRUIMessage`] from raw framed wire bytes (§16).
public func decodeFramedMessage(from data: Data, maxFrameSize: Int = defaultMaxFrameSize) throws -> SRUIMessage {
    let decoder = ProtocolDecoder(maxFrameSize: maxFrameSize)
    return try decoder.decodeFramedMessage(from: data)
}

/// Deserializes a length-delimited [`Transaction`] from raw framed wire bytes (§16).
public func decodeFramedTransaction(from data: Data, limits: StoreLimits = StoreLimits(), maxFrameSize: Int = defaultMaxFrameSize) throws -> Transaction {
    let decoder = ProtocolDecoder(limits: limits, maxFrameSize: maxFrameSize)
    return try decoder.decodeFramedTransaction(from: data)
}

/// Deserializes a length-delimited [`Event`] from raw framed wire bytes (§16).
public func decodeFramedEvent(from data: Data, limits: StoreLimits = StoreLimits(), maxFrameSize: Int = defaultMaxFrameSize) throws -> Event {
    let decoder = ProtocolDecoder(limits: limits, maxFrameSize: maxFrameSize)
    return try decoder.decodeFramedEvent(from: data)
}

/// Serializes a [`Transaction`] to Protobuf bytes (§16).
public func encodeTransaction(_ txn: Transaction) throws -> Data {
    try ProtocolDecoder().encodeTransaction(txn)
}

/// Serializes an [`Operation`] to Protobuf bytes (§16).
public func encodeOperation(_ op: StoreOperation) throws -> Data {
    try ProtocolDecoder().encodeOperation(op)
}

/// Serializes an [`Event`] to Protobuf bytes (§16).
public func encodeEvent(_ event: Event) throws -> Data {
    try ProtocolDecoder().encodeEvent(event)
}

/// Serializes a [`Value`] to Protobuf bytes (§16).
public func encodeValue(_ value: Value) throws -> Data {
    try ProtocolDecoder().encodeValue(value)
}

/// Serializes a [`NodeRecord`] to Protobuf bytes (§16).
public func encodeNodeRecord(_ node: NodeRecord) throws -> Data {
    try ProtocolDecoder().encodeNodeRecord(node)
}

/// Serializes a [`SRUIMessage`] to Protobuf bytes (§16).
public func encodeMessage(_ msg: SRUIMessage) throws -> Data {
    try ProtocolDecoder().encodeMessage(msg)
}

/// Serializes a length-delimited [`SRUIMessage`] frame to Data (§16).
public func encodeFramedMessage(_ msg: SRUIMessage, maxFrameSize: Int = defaultMaxFrameSize) throws -> Data {
    try ProtocolDecoder(maxFrameSize: maxFrameSize).encodeFramedMessage(msg)
}

/// Serializes a length-delimited [`Transaction`] frame to Data (§16).
public func encodeFramedTransaction(_ txn: Transaction, maxFrameSize: Int = defaultMaxFrameSize) throws -> Data {
    try ProtocolDecoder(maxFrameSize: maxFrameSize).encodeFramedTransaction(txn)
}

/// Serializes a length-delimited [`Event`] frame to Data (§16).
public func encodeFramedEvent(_ event: Event, maxFrameSize: Int = defaultMaxFrameSize) throws -> Data {
    try ProtocolDecoder(maxFrameSize: maxFrameSize).encodeFramedEvent(event)
}
