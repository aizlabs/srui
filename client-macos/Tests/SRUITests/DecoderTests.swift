//
// DecoderTests.swift
// SRUITests
//
// Conformance and §26 safety limit tests for pure wire protocol decoding (§16, §22.2, §26).
//

import XCTest
import Foundation
import SwiftProtobuf
@testable import Protocol
@testable import SemanticModel

final class DecoderTests: XCTestCase {
    private var conformanceVectorsDir: URL {
        var current = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        while current.path != "/" {
            let candidate = current.appendingPathComponent("protocol/conformance-vectors")
            if FileManager.default.fileExists(atPath: candidate.path) {
                return candidate
            }
            current = current.deletingLastPathComponent()
        }
        fatalError("Could not locate protocol/conformance-vectors from \(#filePath)")
    }

    private var malformedVectorsDir: URL {
        let dir = conformanceVectorsDir.appendingPathComponent("malformed")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    override func setUpWithError() throws {
        try generateMalformedFixtures()
    }

    // MARK: - Generate Malformed Fixtures

    private func generateMalformedFixtures() throws {
        // 1. Oversized frame (> 16 MiB length prefix)
        let oversizedFrameURL = malformedVectorsDir.appendingPathComponent("oversized_frame.bin")
        // Varint for 33,554,432 (32 MiB): 0x80, 0x80, 0x80, 0x10 followed by payload dummy bytes
        let frameData = Data([0x80, 0x80, 0x80, 0x10, 0x00, 0x01, 0x02, 0x03])
        try frameData.write(to: oversizedFrameURL)

        // 2. Oversized operations in transaction (> limits.maxTransactionOperations)
        let oversizedOpsURL = malformedVectorsDir.appendingPathComponent("oversized_operations.bin")
        var wireTxn = SRUITransaction()
        wireTxn.baseRevision = 1
        wireTxn.newRevision = 2
        var ops: [SRUIOperation] = []
        for i in 1...10005 {
            var op = SRUIOperation()
            var del = Srui_Protocol_DeleteNodeOp()
            del.nodeID = UInt64(i)
            op.deleteNode = del
            ops.append(op)
        }
        wireTxn.operations = ops
        let opsData = try wireTxn.serializedData()
        try opsData.write(to: oversizedOpsURL)

        // 3. Oversized string (> 1 MiB default limit: 1024 * 1024 = 1,048,576 bytes)
        let oversizedStringURL = malformedVectorsDir.appendingPathComponent("oversized_string.bin")
        var wireVal = SRUIValue()
        let hugeString = String(repeating: "A", count: 1024 * 1024 + 100)
        wireVal.stringValue = hugeString
        let strData = try wireVal.serializedData()
        try strData.write(to: oversizedStringURL)

        // 4. Deep nested value (> 10 depth with custom limit 5)
        let deepNestedURL = malformedVectorsDir.appendingPathComponent("deep_nested_value.bin")
        var curr = SRUIValue()
        curr.intValue = 42
        for _ in 0..<15 {
            var parent = SRUIValue()
            var list = Srui_Protocol_ValueList()
            list.values = [curr]
            parent.listValue = list
            curr = parent
        }
        let nestedData = try curr.serializedData()
        try nestedData.write(to: deepNestedURL)

        // 5. Oversized model items (> 10,000 items in model insert)
        let oversizedModelItemsURL = malformedVectorsDir.appendingPathComponent("oversized_model_items.bin")
        var wireOp = SRUIOperation()
        var insert = Srui_Protocol_ModelInsertOp()
        insert.modelID = 1
        insert.index = 0
        var items: [SRUIModelItem] = []
        for i in 1...10005 {
            var item = SRUIModelItem()
            item.itemID = UInt64(i)
            var val = SRUIValue()
            val.intValue = Int64(i)
            item.value = val
            items.append(item)
        }
        insert.items = items
        wireOp.modelInsert = insert
        let modelData = try wireOp.serializedData()
        try modelData.write(to: oversizedModelItemsURL)

        // 6. Corrupted protobuf bytes
        let malformedProtoURL = malformedVectorsDir.appendingPathComponent("malformed_protobuf.bin")
        // Invalid wire type 7 for field 1: (1 << 3) | 7 = 0x0F
        let protoData = Data([0x0F, 0xFF, 0xFF, 0xFF])
        try protoData.write(to: malformedProtoURL)
    }

    // MARK: - Cross-Language Golden Vector Agreement Tests

    func testDecodeGoldenTransactionMatchesTask11DomainModel() throws {
        let fileURL = conformanceVectorsDir.appendingPathComponent("golden_transaction.bin")
        let data = try Data(contentsOf: fileURL)

        let tx = try decodeTransaction(from: data)

        // 1. Transaction metadata
        XCTAssertEqual(tx.baseRevision, Revision(104))
        XCTAssertEqual(tx.newRevision, Revision(105))
        XCTAssertEqual(tx.priority, 1)
        XCTAssertEqual(tx.operations.count, 3)

        // 2. Op 0: CREATE_NODE
        guard case .createNode(let id, let nodeType, let parentID, let childIndex, let props) = tx.operations[0] else {
            XCTFail("Expected createNode operation for op 0")
            return
        }
        XCTAssertEqual(id, NodeId(19))
        XCTAssertEqual(nodeType, TypeRef.text)
        XCTAssertEqual(parentID, NodeId(2))
        XCTAssertEqual(childIndex, 3)
        XCTAssertEqual(props.count, 1)
        XCTAssertEqual(props[0].property, PropertyRef.text)
        XCTAssertEqual(props[0].value, .string("27 tests passed"))

        // 3. Op 1: SET_PROPERTY
        guard case .setProperty(let setNodeId, let setProp, let setVal) = tx.operations[1] else {
            XCTFail("Expected setProperty operation for op 1")
            return
        }
        XCTAssertEqual(setNodeId, NodeId(4))
        XCTAssertEqual(setProp, PropertyRef.value)
        guard case .float64(let f) = setVal else {
            XCTFail("Expected float64 value")
            return
        }
        XCTAssertEqual(f, 0.71, accuracy: 0.0001)

        // 4. Op 2: BATCH_PROPERTY_SET
        guard case .batchPropertySet(let batchNodeId, let batchProps) = tx.operations[2] else {
            XCTFail("Expected batchPropertySet operation for op 2")
            return
        }
        XCTAssertEqual(batchNodeId, NodeId(19))
        XCTAssertEqual(batchProps.count, 1)
        XCTAssertEqual(batchProps[0].property, PropertyRef.minimumSize)
        XCTAssertEqual(batchProps[0].value, .size(Size(width: 120.0, height: 24.0)))

        // 5. Apply decoded Transaction to SemanticStore via TransactionApplier (Task 14 integration)
        var store = SemanticStore(limits: StoreLimits(), revision: Revision(104))
        // Create initial parent nodes 1, 2 (with 3 children so inserting at index 3 succeeds) and node 4 in the store
        let emptyProps: [Property] = []
        try store.apply(StoreOperation.createNode(id: NodeId(1), nodeType: .surface, parentID: nil, childIndex: 0, properties: emptyProps))
        try store.apply(StoreOperation.createNode(id: NodeId(2), nodeType: .surface, parentID: NodeId(1), childIndex: 0, properties: emptyProps))
        try store.apply(StoreOperation.createNode(id: NodeId(20), nodeType: .surface, parentID: NodeId(2), childIndex: 0, properties: emptyProps))
        try store.apply(StoreOperation.createNode(id: NodeId(21), nodeType: .surface, parentID: NodeId(2), childIndex: 1, properties: emptyProps))
        try store.apply(StoreOperation.createNode(id: NodeId(22), nodeType: .surface, parentID: NodeId(2), childIndex: 2, properties: emptyProps))
        try store.apply(StoreOperation.createNode(id: NodeId(4), nodeType: .button, parentID: NodeId(1), childIndex: 1, properties: emptyProps))

        let applier = TransactionApplier(store: store)
        let result = applier.apply(record: tx)
        guard case .success(let newRev) = result else {
            XCTFail("Failed to apply decoded transaction: \(result)")
            return
        }
        XCTAssertEqual(newRev, Revision(105))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(105))

        let committedStore = applier.store
        XCTAssertEqual(committedStore.revision, Revision(105))
        XCTAssertNotNil(committedStore.node(for: NodeId(19)))
        XCTAssertEqual(committedStore.node(for: NodeId(19))?.parentID, NodeId(2))
        XCTAssertEqual(committedStore.node(for: NodeId(19))?.properties[PropertyRef.text], Value.string("27 tests passed"))
        XCTAssertEqual(committedStore.node(for: NodeId(19))?.properties[PropertyRef.minimumSize], Value.size(Size(width: 120.0, height: 24.0)))
        XCTAssertEqual(committedStore.node(for: NodeId(4))?.properties[PropertyRef.value], Value.float64(0.71))
    }

    func testDecodeGoldenNodeRecordMatchesTask11DomainModel() throws {
        let fileURL = conformanceVectorsDir.appendingPathComponent("golden_node_record.bin")
        let data = try Data(contentsOf: fileURL)

        let node = try decodeNodeRecord(from: data)
        XCTAssertEqual(node.nodeId, NodeId(42))
        XCTAssertEqual(node.nodeType, TypeRef.button)
        XCTAssertEqual(node.parentId, NodeId(1))
        XCTAssertEqual(node.childIndex, 0)
        XCTAssertEqual(node.properties.count, 3)

        // Prop 0: label
        XCTAssertEqual(node.properties[0].property, PropertyRef.label)
        XCTAssertEqual(node.properties[0].value, .string("Delete"))

        // Prop 1: role (ActionRole.destructive = enumID 2, valueID 3)
        XCTAssertEqual(node.properties[1].property, PropertyRef.role)
        XCTAssertEqual(node.properties[1].value, .enumToken(EnumToken(enumID: 2, valueID: 3)))

        // Prop 2: enabled
        XCTAssertEqual(node.properties[2].property, PropertyRef.enabled)
        XCTAssertEqual(node.properties[2].value, .bool(true))
    }

    func testDecodeGoldenFramedTransactionMatchesTask11DomainModel() throws {
        let fileURL = conformanceVectorsDir.appendingPathComponent("golden_framed_message.bin")
        let data = try Data(contentsOf: fileURL)

        let tx = try decodeFramedTransaction(from: data)
        XCTAssertEqual(tx.baseRevision, Revision(104))
        XCTAssertEqual(tx.newRevision, Revision(105))
        XCTAssertEqual(tx.priority, 1)
        XCTAssertEqual(tx.operations.count, 3)
    }

    // MARK: - Event Decoding and Symmetrical Encoding Tests

    func testEventDecodingAndEncodingRoundtrip() throws {
        let event = Event(
            clientInstanceId: ClientInstanceId(rawBytes: [0xDE, 0xAD, 0xBE, 0xEF]),
            eventSeq: 999,
            eventId: EventId(rawBytes: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
            observedRevision: Revision(100),
            nodeId: NodeId(42),
            eventType: TypeRef.standard(2),
            arguments: [
                PropertyRef.standard(1): .string("User clicked button"),
                PropertyRef.standard(2): .point(Point(x: 100.5, y: 200.5))
            ]
        )

        // Raw protobuf roundtrip
        let encoded = try encodeEvent(event)
        let decoded = try decodeEvent(from: encoded)
        XCTAssertEqual(event, decoded)

        // Framed message roundtrip
        let framedEncoded = try encodeFramedEvent(event)
        let framedDecoded = try decodeFramedEvent(from: framedEncoded)
        XCTAssertEqual(event, framedDecoded)
    }

    // MARK: - All 17 Value Variants Decoding Roundtrip

    func testAll17ValueVariantsRoundtrip() throws {
        let sampleHash = try ResourceHash(hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        let values: [Value] = [
            .null,
            .bool(true),
            .signedInt(-123456789),
            .unsignedInt(987654321),
            .float64(3.1415926535),
            .string("Swift & Rust Wire Parity"),
            .nodeID(NodeId(101)),
            .itemID(ItemId(202)),
            .resourceHash(sampleHash),
            .enumToken(EnumToken(enumID: 2, valueID: 3)),
            .size(Size(width: 800.0, height: 600.0)),
            .point(Point(x: 10.0, y: 20.0)),
            .range(SemanticRange(start: 5, length: 15)),
            .rect(Rect(x: 0, y: 0, width: 1920, height: 1080)),
            .edgeInsets(EdgeInsets(top: 8, leading: 12, bottom: 8, trailing: 12)),
            .list([.signedInt(1), .string("two"), .bool(true)]),
            .record(SmallRecord(typeRef: TypeRef.standard(1), properties: [
                Property(property: PropertyRef.standard(1), value: .string("nested")),
                Property(property: PropertyRef.standard(2), value: .float64(42.0))
            ]))
        ]

        for val in values {
            let data = try encodeValue(val)
            let decoded = try decodeValue(from: data)
            XCTAssertEqual(val, decoded, "Failed roundtrip for value variant: \(val)")
        }
    }

    // MARK: - All 12 Operation Variants Decoding Roundtrip

    func testAll12OperationVariantsRoundtrip() throws {
        let sampleProps = [Property(property: PropertyRef.standard(1), value: .string("val"))]
        let sampleItems = [ModelItem(itemID: ItemId(1), value: .string("item1"), properties: [PropertyRef.standard(1): .signedInt(10)])]

        let ops: [StoreOperation] = [
            .createNode(id: NodeId(1), nodeType: .surface, parentID: nil, childIndex: 0, properties: sampleProps),
            .deleteNode(id: NodeId(1)),
            .setProperty(id: NodeId(1), property: PropertyRef.standard(1), value: .string("newVal")),
            .clearProperty(id: NodeId(1), property: PropertyRef.standard(1)),
            .moveNode(id: NodeId(1), newParentID: NodeId(2), newChildIndex: 0),
            .reorderChildren(parentID: NodeId(2), newOrder: [NodeId(3), NodeId(1)]),
            .batchPropertySet(id: NodeId(1), properties: sampleProps),
            .createModel(id: ModelId(10), modelType: TypeRef.standard(5), itemCount: 100),
            .modelInsert(id: ModelId(10), index: 0, items: sampleItems),
            .modelDelete(id: ModelId(10), index: 0, count: 1, itemIds: [ItemId(1)]),
            .modelUpdate(id: ModelId(10), index: 0, items: sampleItems),
            .modelResetRange(id: ModelId(10), startIndex: 0, items: sampleItems, totalCount: 100)
        ]

        for op in ops {
            let data = try encodeOperation(op)
            let decoded = try decodeOperation(from: data)
            XCTAssertEqual(op, decoded, "Failed roundtrip for op variant: \(op)")
        }
    }

    // MARK: - Top-Level SruiMessage Wire Variants Roundtrip

    func testTopLevelSruiMessageVariantsRoundtrip() throws {
        // 1. ClientHello
        var helloMsg = SRUIMessage()
        var hello = SRUIClientHello()
        hello.coreVersion = "0.4.0"
        hello.profiles = ["core", "widgets.standard"]
        helloMsg.clientHello = hello

        let helloData = try encodeFramedMessage(helloMsg)
        let helloDecoded = try decodeFramedMessage(from: helloData)
        XCTAssertEqual(helloDecoded.clientHello.coreVersion, "0.4.0")

        // 2. ServerWelcome
        var welcomeMsg = SRUIMessage()
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = "0.4.0"
        welcome.sessionID = "session-12345"
        welcome.initialRevision = 100
        welcomeMsg.serverWelcome = welcome

        let welcomeData = try encodeFramedMessage(welcomeMsg)
        let welcomeDecoded = try decodeFramedMessage(from: welcomeData)
        XCTAssertEqual(welcomeDecoded.serverWelcome.sessionID, "session-12345")
        XCTAssertEqual(welcomeDecoded.serverWelcome.initialRevision, 100)

        // 3. ClientResume
        var resumeMsg = SRUIMessage()
        var resume = SRUIClientResume()
        resume.sessionID = "session-12345"
        resume.lastAppliedRevision = 42
        resumeMsg.clientResume = resume

        let resumeData = try encodeFramedMessage(resumeMsg)
        let resumeDecoded = try decodeFramedMessage(from: resumeData)
        XCTAssertEqual(resumeDecoded.clientResume.lastAppliedRevision, 42)

        // 4. ServerResumeOk
        var resumeOkMsg = SRUIMessage()
        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-12345"
        resumeOk.replayFromRevision = 50
        resumeOkMsg.serverResumeOk = resumeOk

        let resumeOkData = try encodeFramedMessage(resumeOkMsg)
        let resumeOkDecoded = try decodeFramedMessage(from: resumeOkData)
        XCTAssertEqual(resumeOkDecoded.serverResumeOk.replayFromRevision, 50)


        // 5. ServerResyncRequired
        var resyncMsg = SRUIMessage()
        var resync = SRUIServerResyncRequired()
        resync.reason = "Journal evicted"
        resyncMsg.serverResyncRequired = resync

        let resyncData = try encodeFramedMessage(resyncMsg)
        let resyncDecoded = try decodeFramedMessage(from: resyncData)
        XCTAssertEqual(resyncDecoded.serverResyncRequired.reason, "Journal evicted")
    }

    // MARK: - §26 Mandatory Limits Rejection Tests

    func testOversizedFrameRejectedCleanly() throws {
        let fileURL = malformedVectorsDir.appendingPathComponent("oversized_frame.bin")
        let data = try Data(contentsOf: fileURL)

        XCTAssertThrowsError(try decodeFramedMessage(from: data)) { error in
            guard case SRUIFramingError.frameSizeLimitExceeded(let limit, let actual) = error else {
                XCTFail("Expected frameSizeLimitExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, defaultMaxFrameSize)
            XCTAssertEqual(actual, 33_554_432)
        }
    }

    func testOversizedOperationsRejectedCleanlyWithZeroStoreMutation() throws {
        let fileURL = malformedVectorsDir.appendingPathComponent("oversized_operations.bin")
        let data = try Data(contentsOf: fileURL)

        let store = SemanticStore(limits: StoreLimits(), revision: Revision(1))
        let originalSnapshot = store

        // Decode should reject before touching the store
        XCTAssertThrowsError(try decodeTransaction(from: data)) { error in
            guard case ProtocolDecodeError.maxOperationsExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxOperationsExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 10_000)
            XCTAssertEqual(actual, 10_005)
        }

        // Store must remain completely pristine and unmodified
        XCTAssertEqual(store.revision, originalSnapshot.revision)
        XCTAssertEqual(store.nodeCount, originalSnapshot.nodeCount)
    }

    func testOversizedStringRejectedCleanly() throws {
        let fileURL = malformedVectorsDir.appendingPathComponent("oversized_string.bin")
        let data = try Data(contentsOf: fileURL)

        // Default limit is 1024 * 1024 = 1,048,576 bytes; fixture has 1,048,676
        XCTAssertThrowsError(try decodeValue(from: data)) { error in
            guard case ProtocolDecodeError.maxStringLengthExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxStringLengthExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 1024 * 1024)
            XCTAssertEqual(actual, 1024 * 1024 + 100)
        }

        // Also test with tight custom limits
        var customLimits = StoreLimits()
        customLimits.maxStringLength = 10
        var val = SRUIValue()
        val.stringValue = "This string exceeds 10 bytes"
        let customData = try val.serializedData()
        XCTAssertThrowsError(try decodeValue(from: customData, limits: customLimits)) { error in
            guard case ProtocolDecodeError.maxStringLengthExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxStringLengthExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 10)
            XCTAssertEqual(actual, "This string exceeds 10 bytes".utf8.count)
        }
    }

    func testDeepNestedValueRejectedCleanly() throws {
        let fileURL = malformedVectorsDir.appendingPathComponent("deep_nested_value.bin")
        let data = try Data(contentsOf: fileURL)

        // Test with custom limits (max depth 5, fixture has depth 15)
        var customLimits = StoreLimits()
        customLimits.maxValueDepth = 5

        XCTAssertThrowsError(try decodeValue(from: data, limits: customLimits)) { error in
            guard case ProtocolDecodeError.maxValueDepthExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxValueDepthExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 5)
            XCTAssertGreaterThan(actual, 5)
        }
    }

    func testMaxListElementsLimitRejectedCleanly() throws {
        var customLimits = StoreLimits()
        customLimits.maxListElements = 5

        var wireList = Srui_Protocol_ValueList()
        for i in 1...10 {
            var item = SRUIValue()
            item.intValue = Int64(i)
            wireList.values.append(item)
        }
        var wireVal = SRUIValue()
        wireVal.listValue = wireList

        let data = try wireVal.serializedData()
        XCTAssertThrowsError(try decodeValue(from: data, limits: customLimits)) { error in
            guard case ProtocolDecodeError.maxListElementsExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxListElementsExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 5)
            XCTAssertEqual(actual, 10)
        }
    }

    func testMaxRecordPropertiesLimitRejectedCleanly() throws {
        var customLimits = StoreLimits()
        customLimits.maxRecordProperties = 3

        var rec = Srui_Protocol_SmallRecord()
        rec.type = TypeRef.standard(1).toWire()
        for i in 1...5 {
            var prop = SRUIProperty()
            prop.property = PropertyRef.standard(UInt32(i)).toWire()
            var val = SRUIValue()
            val.intValue = Int64(i)
            prop.value = val
            rec.properties.append(prop)
        }
        var wireVal = SRUIValue()
        wireVal.recordValue = rec

        let data = try wireVal.serializedData()
        XCTAssertThrowsError(try decodeValue(from: data, limits: customLimits)) { error in
            guard case ProtocolDecodeError.maxRecordPropertiesExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxRecordPropertiesExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 3)
            XCTAssertEqual(actual, 5)
        }
    }

    func testInvalidResourceHashLengthRejectedCleanly() throws {
        var wireVal = SRUIValue()
        wireVal.resourceHash = Data([0x01, 0x02, 0x03]) // Only 3 bytes instead of 32

        let data = try wireVal.serializedData()
        XCTAssertThrowsError(try decodeValue(from: data)) { error in
            guard case ProtocolDecodeError.invalidResourceHashLength(let len) = error else {
                XCTFail("Expected invalidResourceHashLength, got \(error)")
                return
            }
            XCTAssertEqual(len, 3)
        }
    }

    func testOversizedModelItemsRejectedCleanly() throws {
        let fileURL = malformedVectorsDir.appendingPathComponent("oversized_model_items.bin")
        let data = try Data(contentsOf: fileURL)

        XCTAssertThrowsError(try decodeOperation(from: data)) { error in
            guard case ProtocolDecodeError.maxItemsPerModelOperationExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxItemsPerModelOperationExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 10_000)
            XCTAssertEqual(actual, 10_005)
        }
    }

    func testModelUpdateAndResetRangeItemsLimitRejectedCleanly() throws {
        var customLimits = StoreLimits()
        customLimits.maxItemsPerModelOperation = 2

        var item = SRUIModelItem()
        item.itemID = 1
        var val = SRUIValue()
        val.intValue = 1
        item.value = val

        // 1. ModelUpdate with 3 items
        var updateOp = SRUIOperation()
        var update = Srui_Protocol_ModelUpdateOp()
        update.modelID = 10
        update.items = [item, item, item]
        updateOp.modelUpdate = update

        let updateData = try updateOp.serializedData()
        XCTAssertThrowsError(try decodeOperation(from: updateData, limits: customLimits)) { error in
            guard case ProtocolDecodeError.maxItemsPerModelOperationExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxItemsPerModelOperationExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 2)
            XCTAssertEqual(actual, 3)
        }

        // 2. ModelResetRange with 3 items
        var resetOp = SRUIOperation()
        var reset = Srui_Protocol_ModelResetRangeOp()
        reset.modelID = 10
        reset.items = [item, item, item]
        resetOp.modelResetRange = reset

        let resetData = try resetOp.serializedData()
        XCTAssertThrowsError(try decodeOperation(from: resetData, limits: customLimits)) { error in
            guard case ProtocolDecodeError.maxItemsPerModelOperationExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxItemsPerModelOperationExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 2)
            XCTAssertEqual(actual, 3)
        }
    }

    func testMalformedProtobufRejectedCleanly() throws {
        let fileURL = malformedVectorsDir.appendingPathComponent("malformed_protobuf.bin")
        let data = try Data(contentsOf: fileURL)

        XCTAssertThrowsError(try decodeTransaction(from: data)) { error in
            guard case ProtocolDecodeError.protobufDecodeError = error else {
                XCTFail("Expected protobufDecodeError, got \(error)")
                return
            }
        }
    }

    func testMissingRequiredFieldsRejectedCleanly() throws {
        // 1. Missing node type in NodeRecord
        var wireNode = SRUINodeRecord()
        wireNode.nodeID = 1
        XCTAssertThrowsError(try NodeRecord(wire: wireNode)) { error in
            guard case ProtocolDecodeError.missingField(let field) = error else {
                XCTFail("Expected missingField, got \(error)")
                return
            }
            XCTAssertEqual(field, "NodeRecord.type")
        }

        // 2. Missing op payload in Operation
        let emptyOp = SRUIOperation()
        XCTAssertThrowsError(try StoreOperation(wire: emptyOp)) { error in
            guard case ProtocolDecodeError.missingField(let field) = error else {
                XCTFail("Expected missingField, got \(error)")
                return
            }
            XCTAssertEqual(field, "Operation.op")
        }

        // 3. Missing eventType in Event
        var wireEvent = SRUIEvent()
        wireEvent.nodeID = 10
        XCTAssertThrowsError(try Event(wire: wireEvent)) { error in
            guard case ProtocolDecodeError.missingField(let field) = error else {
                XCTFail("Expected missingField, got \(error)")
                return
            }
            XCTAssertEqual(field, "Event.eventType")
        }
    }

    func testTransactionFailClosedWhenSingleOpViolatesLimit() throws {
        var customLimits = StoreLimits()
        customLimits.maxStringLength = 10

        // Transaction with 3 valid ops and 1 op with an oversized string
        var wireTxn = SRUITransaction()
        wireTxn.baseRevision = 1
        wireTxn.newRevision = 2

        var validOp1 = SRUIOperation()
        var del1 = Srui_Protocol_DeleteNodeOp()
        del1.nodeID = 10
        validOp1.deleteNode = del1

        var invalidOp = SRUIOperation()
        var set = Srui_Protocol_SetPropertyOp()
        set.nodeID = 20
        set.property = PropertyRef.standard(1).toWire()
        var strVal = SRUIValue()
        strVal.stringValue = "This string is way too long for 10 bytes limit"
        set.value = strVal
        invalidOp.setProperty = set

        var validOp2 = SRUIOperation()
        var del2 = Srui_Protocol_DeleteNodeOp()
        del2.nodeID = 30
        validOp2.deleteNode = del2

        wireTxn.operations = [validOp1, invalidOp, validOp2]

        let txnData = try wireTxn.serializedData()

        let store = SemanticStore(limits: customLimits, revision: Revision(1))
        let originalSnapshot = store

        // Decoding MUST fail closed without returning a partial transaction
        XCTAssertThrowsError(try decodeTransaction(from: txnData, limits: customLimits)) { error in
            guard case ProtocolDecodeError.maxStringLengthExceeded(let limit, let actual) = error else {
                XCTFail("Expected maxStringLengthExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 10)
            XCTAssertEqual(actual, "This string is way too long for 10 bytes limit".utf8.count)
        }

        // Store state untouched
        XCTAssertEqual(store.revision, originalSnapshot.revision)
        XCTAssertEqual(store.nodeCount, originalSnapshot.nodeCount)
    }

    // MARK: - §22.2 Thread Safety / Off-Main Execution Test

    func testDecodeOffMainThreadSafety() async throws {
        let authoredTx = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface, parentID: nil, childIndex: 0, properties: [
                    Property(property: .label, value: .string("OffMainThread"))
                ])
            ],
            priority: 0
        )
        let data = try encodeTransaction(authoredTx)

        // Dispatch 100 concurrent decoding tasks off the main actor
        await withTaskGroup(of: Transaction.self) { group in
            for _ in 0..<100 {
                group.addTask {
                    let decoded = try! decodeTransaction(from: data)
                    return decoded
                }
            }

            for await decoded in group {
                XCTAssertEqual(decoded, authoredTx)
            }
        }
    }
}
