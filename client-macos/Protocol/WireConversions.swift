//
// WireConversions.swift
// Protocol
//
// Protobuf wire conversions between in-memory SemanticModel domain types and SwiftProtobuf types (§16).
//

import Foundation
import SwiftProtobuf
import SemanticModel

// MARK: - TypeRef <-> SRUITypeRef

extension TypeRef {
    public init(wire: SRUITypeRef) {
        self.init(namespaceID: wire.namespaceID, localID: wire.localID)
    }

    public func toWire() -> SRUITypeRef {
        var wire = SRUITypeRef()
        wire.namespaceID = self.namespaceID
        wire.localID = self.localID
        return wire
    }
}

// MARK: - PropertyRef <-> SRUIPropertyRef

extension PropertyRef {
    public init(wire: SRUIPropertyRef) {
        self.init(namespaceID: wire.namespaceID, localID: wire.localID)
    }

    public func toWire() -> SRUIPropertyRef {
        var wire = SRUIPropertyRef()
        wire.namespaceID = self.namespaceID
        wire.localID = self.localID
        return wire
    }
}

// MARK: - EnumToken <-> Srui_Protocol_EnumValue

extension EnumToken {
    public init(wire: Srui_Protocol_EnumValue) {
        self.init(enumID: wire.enumID, valueID: wire.valueID)
    }

    public func toWire() -> Srui_Protocol_EnumValue {
        var wire = Srui_Protocol_EnumValue()
        wire.enumID = self.enumID
        wire.valueID = self.valueID
        return wire
    }
}

// MARK: - Geometric & Semantic Tuples <-> Protobuf

extension Size {
    public init(wire: Srui_Protocol_SizeVal) {
        self.init(width: wire.width, height: wire.height)
    }

    public func toWire() -> Srui_Protocol_SizeVal {
        var wire = Srui_Protocol_SizeVal()
        wire.width = self.width
        wire.height = self.height
        return wire
    }
}

extension Point {
    public init(wire: Srui_Protocol_PointVal) {
        self.init(x: wire.x, y: wire.y)
    }

    public func toWire() -> Srui_Protocol_PointVal {
        var wire = Srui_Protocol_PointVal()
        wire.x = self.x
        wire.y = self.y
        return wire
    }
}

extension SemanticRange {
    public init(wire: Srui_Protocol_RangeVal) {
        self.init(start: wire.location, length: wire.length)
    }

    public func toWire() -> Srui_Protocol_RangeVal {
        var wire = Srui_Protocol_RangeVal()
        wire.location = self.start
        wire.length = self.length
        return wire
    }
}

extension Rect {
    public init(wire: Srui_Protocol_RectVal) {
        self.init(x: wire.x, y: wire.y, width: wire.width, height: wire.height)
    }

    public func toWire() -> Srui_Protocol_RectVal {
        var wire = Srui_Protocol_RectVal()
        wire.x = self.x
        wire.y = self.y
        wire.width = self.width
        wire.height = self.height
        return wire
    }
}

extension EdgeInsets {
    public init(wire: Srui_Protocol_EdgeInsetsVal) {
        self.init(top: wire.top, leading: wire.leading, bottom: wire.bottom, trailing: wire.trailing)
    }

    public func toWire() -> Srui_Protocol_EdgeInsetsVal {
        var wire = Srui_Protocol_EdgeInsetsVal()
        wire.top = self.top
        wire.leading = self.leading
        wire.bottom = self.bottom
        wire.trailing = self.trailing
        return wire
    }
}

// MARK: - Property <-> SRUIProperty

extension Property {
    public init(wire: SRUIProperty) throws {
        guard wire.hasProperty else {
            throw ValueConversionError.missingField("property")
        }
        let prop = PropertyRef(wire: wire.property)
        let val: Value
        if wire.hasValue {
            val = try Value(wire: wire.value)
        } else {
            val = .null
        }
        self.init(property: prop, value: val)
    }

    public func toWire() -> SRUIProperty {
        var wire = SRUIProperty()
        wire.property = property.toWire()
        wire.value = value.toWire()
        return wire
    }
}

// MARK: - SmallRecord <-> Srui_Protocol_SmallRecord

extension SmallRecord {
    public init(wire: Srui_Protocol_SmallRecord) throws {
        guard wire.hasType else {
            throw ValueConversionError.missingField("record.type")
        }
        let typeRef = TypeRef(wire: wire.type)
        var props: [Property] = []
        props.reserveCapacity(wire.properties.count)
        for p in wire.properties {
            props.append(try Property(wire: p))
        }
        self.init(typeRef: typeRef, properties: props)
    }

    public func toWire() -> Srui_Protocol_SmallRecord {
        var wire = Srui_Protocol_SmallRecord()
        wire.type = typeRef.toWire()
        wire.properties = properties.map { $0.toWire() }
        return wire
    }
}

// MARK: - Value <-> SRUIValue

extension Value {
    public init(wire: SRUIValue) throws {
        guard let wireVal = wire.value else {
            throw ValueConversionError.missingField("value")
        }

        switch wireVal {
        case .nullValue:
            self = .null
        case .boolValue(let b):
            self = .bool(b)
        case .intValue(let i):
            self = .signedInt(i)
        case .uintValue(let u):
            self = .unsignedInt(u)
        case .floatValue(let f):
            self = .float64(f)
        case .stringValue(let s):
            self = .string(s)
        case .nodeIDValue(let id):
            self = .nodeID(NodeId(id))
        case .itemIDValue(let id):
            self = .itemID(ItemId(id))
        case .resourceHash(let bytes):
            guard bytes.count == 32 else {
                throw ValueConversionError.invalidResourceHashLength(bytes.count)
            }
            guard let hash = try? ResourceHash(bytes: bytes) else {
                throw ValueConversionError.invalidResourceHashLength(bytes.count)
            }
            self = .resourceHash(hash)
        case .enumValue(let e):
            self = .enumToken(EnumToken(wire: e))
        case .sizeValue(let s):
            self = .size(Size(wire: s))
        case .pointValue(let p):
            self = .point(Point(wire: p))
        case .rangeValue(let r):
            self = .range(SemanticRange(wire: r))
        case .rectValue(let r):
            self = .rect(Rect(wire: r))
        case .insetsValue(let i):
            self = .edgeInsets(EdgeInsets(wire: i))
        case .listValue(let l):
            var items: [Value] = []
            items.reserveCapacity(l.values.count)
            for v in l.values {
                items.append(try Value(wire: v))
            }
            self = .list(items)
        case .recordValue(let r):
            self = .record(try SmallRecord(wire: r))
        }
    }

    public func toWire() -> SRUIValue {
        var wire = SRUIValue()
        switch self {
        case .null:
            wire.nullValue = .nullValue
        case .bool(let b):
            wire.boolValue = b
        case .signedInt(let i):
            wire.intValue = i
        case .unsignedInt(let u):
            wire.uintValue = u
        case .float64(let f):
            wire.floatValue = f
        case .string(let s):
            wire.stringValue = s
        case .nodeID(let id):
            wire.nodeIDValue = id.value
        case .itemID(let id):
            wire.itemIDValue = id.value
        case .resourceHash(let h):
            wire.resourceHash = h.bytes
        case .enumToken(let e):
            wire.enumValue = e.toWire()
        case .size(let s):
            wire.sizeValue = s.toWire()
        case .point(let p):
            wire.pointValue = p.toWire()
        case .range(let r):
            wire.rangeValue = r.toWire()
        case .rect(let r):
            wire.rectValue = r.toWire()
        case .edgeInsets(let i):
            wire.insetsValue = i.toWire()
        case .list(let list):
            var wireList = Srui_Protocol_ValueList()
            wireList.values = list.map { $0.toWire() }
            wire.listValue = wireList
        case .record(let rec):
            wire.recordValue = rec.toWire()
        }
        return wire
    }
}

// MARK: - ModelItem <-> SRUIModelItem

extension ModelItem {
    public init(wire: SRUIModelItem) throws {
        let itemID = ItemId(wire.itemID)
        let val = wire.hasValue ? try Value(wire: wire.value) : .null
        var props: [PropertyRef: Value] = [:]
        props.reserveCapacity(wire.properties.count)
        for p in wire.properties {
            let prop = try Property(wire: p)
            props[prop.property] = prop.value
        }
        self.init(itemID: itemID, value: val, properties: props)
    }

    public func toWire() -> SRUIModelItem {
        var wire = SRUIModelItem()
        wire.itemID = itemID.value
        wire.value = value.toWire()
        wire.properties = properties.map { (k, v) in
            var wireProp = SRUIProperty()
            wireProp.property = k.toWire()
            wireProp.value = v.toWire()
            return wireProp
        }
        return wire
    }
}

// MARK: - NodeRecord <-> SRUINodeRecord

extension NodeRecord {
    public init(wire: SRUINodeRecord) throws {
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
            props.append(try Property(wire: p))
        }
        self.init(
            nodeId: nodeId,
            nodeType: nodeType,
            parentId: parentId,
            childIndex: childIndex,
            properties: props
        )
    }

    public func toWire() -> SRUINodeRecord {
        var wire = SRUINodeRecord()
        wire.nodeID = nodeId.value
        wire.type = nodeType.toWire()
        wire.parentID = parentId?.value ?? 0
        wire.childIndex = childIndex.map { UInt32($0) } ?? UInt32.max
        wire.properties = properties.map { $0.toWire() }
        return wire
    }
}

// MARK: - Operation <-> SRUIOperation

extension StoreOperation {
    public init(wire: SRUIOperation) throws {
        guard let op = wire.op else {
            throw ProtocolDecodeError.missingField("Operation.op")
        }

        switch op {
        case .createNode(let create):
            guard create.hasNode else {
                throw ProtocolDecodeError.missingField("CreateNodeOp.node")
            }
            let record = try NodeRecord(wire: create.node)
            self = .createNode(
                id: record.nodeId,
                nodeType: record.nodeType,
                parentID: record.parentId,
                childIndex: record.childIndex,
                properties: record.properties
            )

        case .deleteNode(let del):
            self = .deleteNode(id: NodeId(del.nodeID))

        case .setProperty(let set):
            guard set.hasProperty else {
                throw ProtocolDecodeError.missingField("SetPropertyOp.property")
            }
            let id = NodeId(set.nodeID)
            let propRef = PropertyRef(wire: set.property)
            let val = set.hasValue ? try Value(wire: set.value) : .null
            self = .setProperty(id: id, property: propRef, value: val)

        case .clearProperty_p(let clear):
            guard clear.hasProperty else {
                throw ProtocolDecodeError.missingField("ClearPropertyOp.property")
            }
            self = .clearProperty(id: NodeId(clear.nodeID), property: PropertyRef(wire: clear.property))

        case .moveNode(let move):
            let id = NodeId(move.nodeID)
            let newParentID = move.newParentID == 0 ? nil : NodeId(move.newParentID)
            let newChildIndex = move.newChildIndex == UInt32.max ? nil : Int(move.newChildIndex)
            self = .moveNode(id: id, newParentID: newParentID, newChildIndex: newChildIndex)

        case .reorderChildren(let reorder):
            let parentID = NodeId(reorder.parentID)
            let newOrder = reorder.childNodeIds.map { NodeId($0) }
            self = .reorderChildren(parentID: parentID, newOrder: newOrder)

        case .batchPropertySet(let batch):
            let id = NodeId(batch.nodeID)
            var props: [Property] = []
            props.reserveCapacity(batch.properties.count)
            for p in batch.properties {
                props.append(try Property(wire: p))
            }
            self = .batchPropertySet(id: id, properties: props)

        case .createModel(let create):
            let id = ModelId(create.modelID)
            guard create.hasModelType else {
                throw ProtocolDecodeError.missingField("CreateModelOp.modelType")
            }
            let modelType = TypeRef(wire: create.modelType)
            self = .createModel(id: id, modelType: modelType, itemCount: create.itemCount)

        case .modelInsert(let insert):
            let id = ModelId(insert.modelID)
            var items: [ModelItem] = []
            items.reserveCapacity(insert.items.count)
            for item in insert.items {
                items.append(try ModelItem(wire: item))
            }
            self = .modelInsert(id: id, index: insert.index, items: items)

        case .modelDelete(let del):
            let id = ModelId(del.modelID)
            let index = del.count > 0 ? del.index : nil
            let count = del.count > 0 ? del.count : nil
            let itemIds = del.itemIds.map { ItemId($0) }
            self = .modelDelete(id: id, index: index, count: count, itemIds: itemIds)

        case .modelUpdate(let update):
            let id = ModelId(update.modelID)
            let index = update.index == UInt64.max ? nil : update.index
            var items: [ModelItem] = []
            items.reserveCapacity(update.items.count)
            for item in update.items {
                items.append(try ModelItem(wire: item))
            }
            self = .modelUpdate(id: id, index: index, items: items)

        case .modelResetRange(let reset):
            let id = ModelId(reset.modelID)
            let totalCount = reset.totalCount > 0 ? reset.totalCount : nil
            var items: [ModelItem] = []
            items.reserveCapacity(reset.items.count)
            for item in reset.items {
                items.append(try ModelItem(wire: item))
            }
            self = .modelResetRange(id: id, startIndex: reset.startIndex, items: items, totalCount: totalCount)

        default:
            throw ProtocolDecodeError.invalidOperation("Unsupported or unknown operation variant in wire envelope")
        }
    }

    public func toWire() -> SRUIOperation {
        var op = SRUIOperation()
        switch self {
        case .createNode(let id, let nodeType, let parentID, let childIndex, let properties):
            var create = Srui_Protocol_CreateNodeOp()
            var record = Srui_Protocol_NodeRecord()
            record.nodeID = id.value
            record.type = nodeType.toWire()
            record.parentID = parentID?.value ?? 0
            record.childIndex = childIndex.map { UInt32($0) } ?? UInt32.max
            record.properties = properties.map { $0.toWire() }
            create.node = record
            op.createNode = create

        case .deleteNode(let id):
            var del = Srui_Protocol_DeleteNodeOp()
            del.nodeID = id.value
            op.deleteNode = del

        case .setProperty(let id, let prop, let val):
            var set = Srui_Protocol_SetPropertyOp()
            set.nodeID = id.value
            set.property = prop.toWire()
            set.value = val.toWire()
            op.setProperty = set

        case .clearProperty(let id, let prop):
            var clear = Srui_Protocol_ClearPropertyOp()
            clear.nodeID = id.value
            clear.property = prop.toWire()
            op.clearProperty_p = clear

        case .moveNode(let id, let newParentID, let newChildIndex):
            var move = Srui_Protocol_MoveNodeOp()
            move.nodeID = id.value
            move.newParentID = newParentID?.value ?? 0
            move.newChildIndex = newChildIndex.map { UInt32($0) } ?? UInt32.max
            op.moveNode = move

        case .reorderChildren(let parentID, let newOrder):
            var reorder = Srui_Protocol_ReorderChildrenOp()
            reorder.parentID = parentID.value
            reorder.childNodeIds = newOrder.map { $0.value }
            op.reorderChildren = reorder

        case .batchPropertySet(let id, let properties):
            var batch = Srui_Protocol_BatchPropertySetOp()
            batch.nodeID = id.value
            batch.properties = properties.map { $0.toWire() }
            op.batchPropertySet = batch

        case .createModel(let id, let modelType, let itemCount):
            var create = Srui_Protocol_CreateModelOp()
            create.modelID = id.value
            create.modelType = modelType.toWire()
            create.itemCount = itemCount
            op.createModel = create

        case .modelInsert(let id, let index, let items):
            var insert = Srui_Protocol_ModelInsertOp()
            insert.modelID = id.value
            insert.index = index
            insert.items = items.map { $0.toWire() }
            op.modelInsert = insert

        case .modelDelete(let id, let index, let count, let itemIds):
            var del = Srui_Protocol_ModelDeleteOp()
            del.modelID = id.value
            del.index = index ?? 0
            del.count = count ?? 0
            del.itemIds = itemIds.map { $0.value }
            op.modelDelete = del

        case .modelUpdate(let id, let index, let items):
            var update = Srui_Protocol_ModelUpdateOp()
            update.modelID = id.value
            update.index = index ?? UInt64.max
            update.items = items.map { $0.toWire() }
            op.modelUpdate = update

        case .modelResetRange(let id, let startIndex, let items, let totalCount):
            var reset = Srui_Protocol_ModelResetRangeOp()
            reset.modelID = id.value
            reset.startIndex = startIndex
            reset.totalCount = totalCount ?? 0
            reset.items = items.map { $0.toWire() }
            op.modelResetRange = reset
        }
        return op
    }
}

// MARK: - Transaction <-> SRUITransaction

extension Transaction {
    public init(wire: SRUITransaction) throws {
        var ops: [StoreOperation] = []
        ops.reserveCapacity(wire.operations.count)
        for op in wire.operations {
            ops.append(try StoreOperation(wire: op))
        }
        self.init(
            baseRevision: Revision(wire.baseRevision),
            newRevision: Revision(wire.newRevision),
            operations: ops,
            priority: wire.priority
        )
    }

    public func toWire() -> SRUITransaction {
        var wire = SRUITransaction()
        wire.baseRevision = baseRevision.value
        wire.newRevision = newRevision.value
        wire.priority = priority
        wire.operations = operations.map { $0.toWire() }
        return wire
    }
}

// MARK: - Event <-> SRUIEvent

extension Event {
    public init(wire: SRUIEvent) throws {
        let clientInstanceId = wire.clientInstanceID.isEmpty ? nil : ClientInstanceId(wire.clientInstanceID)
        guard wire.hasEventType else {
            throw ProtocolDecodeError.missingField("Event.eventType")
        }
        let eventType = TypeRef(wire: wire.eventType)

        var args: [PropertyRef: Value] = [:]
        args.reserveCapacity(wire.arguments.count)
        for p in wire.arguments {
            let prop = try Property(wire: p)
            args[prop.property] = prop.value
        }

        self.init(
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

    public func toWire() -> SRUIEvent {
        var wire = SRUIEvent()
        if let cid = clientInstanceId {
            wire.clientInstanceID = cid.bytes
        }
        wire.eventSeq = eventSeq
        wire.eventID = eventId.bytes
        wire.observedRevision = observedRevision.value
        wire.nodeID = nodeId.value
        wire.eventType = eventType.toWire()
        wire.arguments = arguments.map { (k, v) in
            var wireProp = SRUIProperty()
            wireProp.property = k.toWire()
            wireProp.value = v.toWire()
            return wireProp
        }
        wire.editSeq = editSeq?.rawValue ?? 0
        return wire
    }
}

// MARK: - TransactionApplier & SemanticStore Wire Integration

extension TransactionApplier {
    /// Decodes a protobuf wire `SRUITransaction` under the store's §26 limits.
    private func decode(wire: SRUITransaction) -> Result<Transaction, TxnError> {
        do {
            let data = try wire.serializedData()
            return .success(try decodeTransaction(from: data, limits: store.limits))
        } catch let error as ProtocolDecodeError {
            return .failure(.wireError(error.description))
        } catch {
            return .failure(.wireError(String(describing: error)))
        }
    }

    /// Decodes and applies a protobuf wire `SRUITransaction` as an authoritative commit
    /// (§12.1, §16).
    ///
    /// Accepts only `newRevision == baseRevision + 1`. A live stream also carries coalesced scalar
    /// deltas spanning several revisions, which belong to `applyDelivered(wire:)` (§12.1, §20.4).
    public func apply(wire: SRUITransaction) -> Result<Revision, TxnError> {
        decode(wire: wire).flatMap { apply(record: $0) }
    }

    /// Decodes and applies one frame of the live stream to a replica (§12.1, §20.4).
    ///
    /// Accepts both delivery forms — an authoritative commit and a coalesced scalar delta — which
    /// is what a client consuming a server stream needs; a resync snapshot is deliberately not
    /// accepted here (§18).
    public func applyDelivered(wire: SRUITransaction) -> Result<Revision, TxnError> {
        decode(wire: wire).flatMap { applyDelivered(record: $0).map(\.revision) }
    }
}

extension SemanticStore {
    /// Applies a protobuf wire operation directly to this store (§13, §16).
    public mutating func apply(wire: SRUIOperation) throws {
        let data = try wire.serializedData()
        let op = try decodeOperation(from: data, limits: limits)
        try apply(op)
    }

    /// Applies a list of protobuf wire operations atomically (§12.1, §16).
    public mutating func apply(wireOperations: [SRUIOperation]) throws {
        let ops = try wireOperations.map { wireOp in
            let data = try wireOp.serializedData()
            return try decodeOperation(from: data, limits: limits)
        }
        try apply(ops)
    }

    /// Decodes and applies a protobuf wire `SRUITransaction` as an authoritative commit
    /// (§12.1, §16).
    public mutating func applyWireTransaction(_ wire: SRUITransaction) -> Result<Revision, TxnError> {
        applyWire(wire) { applier in applier.apply(wire: wire) }
    }

    /// Decodes and applies one frame of the live stream, accepting either delivery form
    /// (§12.1, §20.4).
    public mutating func applyDeliveredWireTransaction(
        _ wire: SRUITransaction
    ) -> Result<Revision, TxnError> {
        applyWire(wire) { applier in applier.applyDelivered(wire: wire) }
    }

    /// Runs one applier entry point against a temporary applier, adopting its store only on success.
    private mutating func applyWire(
        _ wire: SRUITransaction,
        _ body: (TransactionApplier) -> Result<Revision, TxnError>
    ) -> Result<Revision, TxnError> {
        let applier = TransactionApplier(store: self)
        let res = body(applier)
        if case .success = res {
            self = applier.currentSnapshot.store
        }
        return res
    }
}
