//
// SemanticStoreTests.swift
// SemanticModelTests
//
// Integration and invariant tests for client-side SemanticStore replica (§6.2, §6.3, §13, §26).
//

import XCTest
import Foundation
@testable import Protocol
@testable import SemanticModel

final class SemanticStoreTests: XCTestCase {

    // MARK: - Tree Construction & Hierarchy (Mirroring Task 4)

    func testTreeConstructionAndHierarchy() throws {
        var store = SemanticStore()

        // 1. Create root Surface (#1)
        let rootID = NodeId(1)
        try store.createNode(
            id: rootID,
            nodeType: .surface,
            properties: [(.label, .string("App Window"))]
        )

        // 2. Create Column (#2) under root (#1)
        let colID = NodeId(2)
        try store.createNode(
            id: colID,
            nodeType: .column,
            parentID: rootID,
            properties: [(.spacingRole, .unsignedInt(2))]
        )

        // 3. Create Text (#3) and Button (#4) under Column (#2)
        let textID = NodeId(3)
        try store.createNode(
            id: textID,
            nodeType: .text,
            parentID: colID,
            properties: [(.text, .string("Hello SRUI"))]
        )

        let buttonID = NodeId(4)
        try store.createNode(
            id: buttonID,
            nodeType: .button,
            parentID: colID,
            properties: [
                (.label, .string("Click Me")),
                (.enabled, .bool(true))
            ]
        )

        // Assert shape
        XCTAssertEqual(store.nodeCount, 4)
        XCTAssertEqual(store.rootIDs, [rootID])
        XCTAssertEqual(store.children(of: rootID), [colID])
        XCTAssertEqual(store.children(of: colID), [textID, buttonID])
        XCTAssertEqual(store.children(of: textID), [])
        XCTAssertEqual(store.children(of: buttonID), [])

        XCTAssertEqual(store.nodeDepth(rootID), 1)
        XCTAssertEqual(store.nodeDepth(colID), 2)
        XCTAssertEqual(store.nodeDepth(textID), 3)
        XCTAssertEqual(store.nodeDepth(buttonID), 3)
        XCTAssertEqual(store.subtreeDepth(rootID), 3)

        // Verify property access
        let textNode = try XCTUnwrap(store.getNode(textID))
        XCTAssertEqual(textNode.getProperty(.text), .string("Hello SRUI"))

        let btnNode = try XCTUnwrap(store.getNode(buttonID))
        XCTAssertEqual(btnNode.getProperty(.label), .string("Click Me"))
        XCTAssertEqual(btnNode.getProperty(.enabled), .bool(true))
    }

    // MARK: - Property Mutations (Set, Clear, Batch)

    func testPropertyMutationsSetClearBatch() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        try store.createNode(id: rootID, nodeType: .surface)

        // Set property
        let prev = try store.setProperty(nodeID: rootID, property: .label, value: .string("Initial Title"))
        XCTAssertNil(prev)
        XCTAssertEqual(store.getNode(rootID)?.getProperty(.label), .string("Initial Title"))

        // Update property
        let updatedPrev = try store.setProperty(nodeID: rootID, property: .label, value: .string("Updated Title"))
        XCTAssertEqual(updatedPrev, .string("Initial Title"))
        XCTAssertEqual(store.getNode(rootID)?.getProperty(.label), .string("Updated Title"))

        // Batch set properties
        try store.batchPropertySet(
            nodeID: rootID,
            properties: [
                (.enabled, .bool(false)),
                (.busy, .bool(true))
            ]
        )
        let node = try XCTUnwrap(store.getNode(rootID))
        XCTAssertEqual(node.getProperty(.enabled), .bool(false))
        XCTAssertEqual(node.getProperty(.busy), .bool(true))

        // Clear property
        let removed = try store.clearProperty(nodeID: rootID, property: .busy)
        XCTAssertEqual(removed, .bool(true))
        XCTAssertNil(store.getNode(rootID)?.getProperty(.busy))
    }

    // MARK: - Reorder Children

    func testReorderChildren() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let colID = NodeId(2)
        let c1 = NodeId(10)
        let c2 = NodeId(20)
        let c3 = NodeId(30)

        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: colID, nodeType: .column, parentID: rootID)
        try store.createNode(id: c1, nodeType: .text, parentID: colID)
        try store.createNode(id: c2, nodeType: .text, parentID: colID)
        try store.createNode(id: c3, nodeType: .text, parentID: colID)

        XCTAssertEqual(store.children(of: colID), [c1, c2, c3])

        // Reorder children to [c3, c1, c2]
        try store.reorderChildren(parentID: colID, newOrder: [c3, c1, c2])
        XCTAssertEqual(store.children(of: colID), [c3, c1, c2])

        // Reject mismatch length
        XCTAssertThrowsError(try store.reorderChildren(parentID: colID, newOrder: [c3, c1])) { error in
            guard case StoreError.invalidChildrenReorder = error else {
                XCTFail("Expected invalidChildrenReorder, got \(error)")
                return
            }
        }

        // Reject foreign child ID
        let foreign = NodeId(999)
        XCTAssertThrowsError(try store.reorderChildren(parentID: colID, newOrder: [c3, c1, foreign])) { error in
            guard case StoreError.invalidChildrenReorder = error else {
                XCTFail("Expected invalidChildrenReorder, got \(error)")
                return
            }
        }

        // Reject duplicates
        XCTAssertThrowsError(try store.reorderChildren(parentID: colID, newOrder: [c3, c1, c1])) { error in
            guard case StoreError.invalidChildrenReorder = error else {
                XCTFail("Expected invalidChildrenReorder, got \(error)")
                return
            }
        }

        // State remains unaffected by rejected reorders
        XCTAssertEqual(store.children(of: colID), [c3, c1, c2])
    }

    // MARK: - Move Node

    func testMoveNode() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let col1ID = NodeId(2)
        let col2ID = NodeId(3)
        let itemID = NodeId(4)
        let itemChildID = NodeId(5)

        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: col1ID, nodeType: .column, parentID: rootID)
        try store.createNode(id: col2ID, nodeType: .column, parentID: rootID)
        try store.createNode(id: itemID, nodeType: .row, parentID: col1ID)
        try store.createNode(id: itemChildID, nodeType: .text, parentID: itemID)

        XCTAssertEqual(store.children(of: col1ID), [itemID])
        XCTAssertEqual(store.children(of: col2ID), [])
        XCTAssertEqual(store.nodeDepth(itemChildID), 4)

        // Move itemID from col1 to col2
        try store.moveNode(nodeID: itemID, newParentID: col2ID, newChildIndex: nil)

        XCTAssertEqual(store.children(of: col1ID), [])
        XCTAssertEqual(store.children(of: col2ID), [itemID])
        XCTAssertEqual(store.parent(of: itemID), col2ID)
        XCTAssertEqual(store.nodeDepth(itemChildID), 4)

        // Move itemID to root (parent = nil)
        try store.moveNode(nodeID: itemID, newParentID: nil, newChildIndex: nil)
        XCTAssertEqual(store.children(of: col2ID), [])
        XCTAssertEqual(store.parent(of: itemID), .some(nil))
        XCTAssertEqual(store.nodeDepth(itemID), 1)
        XCTAssertEqual(store.nodeDepth(itemChildID), 2)
    }

    func testMoveNodeSameParentAppendIndex() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let colID = NodeId(2)
        let c1 = NodeId(3)
        let c2 = NodeId(4)
        let c3 = NodeId(5)

        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: colID, nodeType: .column, parentID: rootID)
        try store.createNode(id: c1, nodeType: .text, parentID: colID, childIndex: 0)
        try store.createNode(id: c2, nodeType: .text, parentID: colID, childIndex: 1)
        try store.createNode(id: c3, nodeType: .text, parentID: colID, childIndex: 2)

        XCTAssertEqual(store.children(of: colID), [c1, c2, c3])

        let appendIndex = store.children(of: colID)!.count
        try store.moveNode(nodeID: c2, newParentID: colID, newChildIndex: appendIndex)

        XCTAssertEqual(store.children(of: colID), [c1, c3, c2])
    }

    func testMoveNodeCyclePrevention() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let parentID = NodeId(2)
        let childID = NodeId(3)
        let grandchildID = NodeId(4)

        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: parentID, nodeType: .column, parentID: rootID)
        try store.createNode(id: childID, nodeType: .row, parentID: parentID)
        try store.createNode(id: grandchildID, nodeType: .text, parentID: childID)

        // Moving parent under itself -> Cycle
        XCTAssertThrowsError(try store.moveNode(nodeID: parentID, newParentID: parentID, newChildIndex: nil)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.cycleDetected(nodeID: parentID, targetParent: parentID))
        }

        // Moving parent under its grandchild -> Cycle
        XCTAssertThrowsError(try store.moveNode(nodeID: parentID, newParentID: grandchildID, newChildIndex: nil)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.cycleDetected(nodeID: parentID, targetParent: grandchildID))
        }

        // Tree structure remains unmodified
        XCTAssertEqual(store.parent(of: parentID), parentID == 1 ? .some(nil) : .some(rootID))
        XCTAssertEqual(store.parent(of: grandchildID), .some(childID))
    }

    // MARK: - Delete Node Recursive Subtree

    func testDeleteNodeRecursiveSubtreeCleanup() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let colID = NodeId(2)
        let c1 = NodeId(3)
        let c2 = NodeId(4)
        let gc1 = NodeId(5)

        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: colID, nodeType: .column, parentID: rootID)
        try store.createNode(id: c1, nodeType: .row, parentID: colID)
        try store.createNode(id: gc1, nodeType: .text, parentID: c1)
        try store.createNode(id: c2, nodeType: .button, parentID: colID)

        XCTAssertEqual(store.nodeCount, 5)

        // Delete subtree rooted at c1 (#3)
        let deleted = try store.deleteNode(c1)
        XCTAssertEqual(deleted.count, 2)
        XCTAssertTrue(deleted.contains(c1))
        XCTAssertTrue(deleted.contains(gc1))

        // Active nodes count is reduced
        XCTAssertEqual(store.nodeCount, 3)
        XCTAssertFalse(store.containsNode(c1))
        XCTAssertFalse(store.containsNode(gc1))
        XCTAssertTrue(store.containsNode(c2))
        XCTAssertEqual(store.children(of: colID), [c2])
    }

    // MARK: - Invariant: NodeId Reuse (§6.2)

    func testNodeIdCannotBeReusedEvenAfterDeletion() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let itemID = NodeId(42)

        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: itemID, nodeType: .button, parentID: rootID)

        // 1. Attempting duplicate CREATE_NODE while active is rejected (§6.2)
        XCTAssertThrowsError(try store.createNode(id: itemID, nodeType: .button, parentID: rootID)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.nodeIdAlreadyUsed(itemID))
        }

        // 2. Delete the node
        try store.deleteNode(itemID)
        XCTAssertFalse(store.containsNode(itemID))
        XCTAssertTrue(store.isIDUsed(itemID))

        // 3. Attempting to create a node with the deleted ID is STILL rejected (§6.2)
        XCTAssertThrowsError(try store.createNode(id: itemID, nodeType: .text, parentID: rootID)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.nodeIdAlreadyUsed(itemID))
        }
    }

    func testCreateUnderNonexistentParentRejected() {
        var store = SemanticStore()
        let fakeParent = NodeId(999)
        let childID = NodeId(1)

        XCTAssertThrowsError(try store.createNode(id: childID, nodeType: .button, parentID: fakeParent)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.parentNotFound(fakeParent))
        }

        XCTAssertTrue(store.isEmpty)
        XCTAssertFalse(store.isIDUsed(childID))
    }

    // MARK: - Invariant Limits (§26)

    func testMaxTreeDepthEnforcedAndStoreUnchanged() throws {
        let limits = StoreLimits(maxTreeDepth: 3, maxNodeCount: 1000, maxStringLength: 1024)
        var store = SemanticStore(limits: limits)

        let n1 = NodeId(1) // depth 1
        let n2 = NodeId(2) // depth 2
        let n3 = NodeId(3) // depth 3
        let n4 = NodeId(4) // depth 4 -> should fail

        try store.createNode(id: n1, nodeType: .surface)
        try store.createNode(id: n2, nodeType: .column, parentID: n1)
        try store.createNode(id: n3, nodeType: .row, parentID: n2)

        XCTAssertEqual(store.nodeDepth(n3), 3)
        XCTAssertEqual(store.nodeCount, 3)

        // Creating n4 under n3 would produce depth 4 > 3
        XCTAssertThrowsError(try store.createNode(id: n4, nodeType: .text, parentID: n3)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxTreeDepthExceeded(limit: 3, actual: 4))
        }

        // Store is left completely unchanged
        XCTAssertEqual(store.nodeCount, 3)
        XCTAssertFalse(store.containsNode(n4))
        XCTAssertFalse(store.isIDUsed(n4))
        XCTAssertEqual(store.children(of: n3), [])

        // Moving a subtree that would exceed max depth is also rejected
        let otherRoot = NodeId(10) // depth 1
        let otherChild = NodeId(11) // depth 2
        try store.createNode(id: otherRoot, nodeType: .surface)
        try store.createNode(id: otherChild, nodeType: .button, parentID: otherRoot)

        // moving otherRoot (height 2) under n3 (depth 3) would result in depth 3 + 2 = 5 > 3
        XCTAssertThrowsError(try store.moveNode(nodeID: otherRoot, newParentID: n3, newChildIndex: nil)) { error in
            guard case StoreError.maxTreeDepthExceeded = error else {
                XCTFail("Expected maxTreeDepthExceeded, got \(error)")
                return
            }
        }
        XCTAssertEqual(store.parent(of: otherRoot), .some(nil))
    }

    func testMaxNodeCountEnforcedAndStoreUnchanged() throws {
        let limits = StoreLimits(maxTreeDepth: 64, maxNodeCount: 2, maxStringLength: 1024)
        var store = SemanticStore(limits: limits)

        let n1 = NodeId(1)
        let n2 = NodeId(2)
        let n3 = NodeId(3)

        try store.createNode(id: n1, nodeType: .surface)
        try store.createNode(id: n2, nodeType: .button, parentID: n1)

        XCTAssertEqual(store.nodeCount, 2)

        // Adding 3rd node exceeds limit of 2
        XCTAssertThrowsError(try store.createNode(id: n3, nodeType: .text, parentID: n1)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxNodeCountExceeded(limit: 2, current: 2))
        }

        // Store is left completely unchanged
        XCTAssertEqual(store.nodeCount, 2)
        XCTAssertFalse(store.containsNode(n3))
        XCTAssertFalse(store.isIDUsed(n3))
        XCTAssertEqual(store.children(of: n1), [n2])
    }

    func testMaxStringLengthEnforcedAndStoreUnchanged() throws {
        let limits = StoreLimits(maxTreeDepth: 64, maxNodeCount: 1000, maxStringLength: 10)
        var store = SemanticStore(limits: limits)

        let n1 = NodeId(1)

        // Valid string (<= 10 bytes)
        try store.createNode(
            id: n1,
            nodeType: .surface,
            properties: [(.label, .string("Short"))]
        )

        // Exceeding on setProperty
        XCTAssertThrowsError(
            try store.setProperty(nodeID: n1, property: .label, value: .string("This is way too long string!"))
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxStringLengthExceeded(limit: 10, actual: 28))
        }

        // Property in store is unchanged
        XCTAssertEqual(store.getNode(n1)?.getProperty(.label), .string("Short"))

        // Exceeding on createNode
        let n2 = NodeId(2)
        XCTAssertThrowsError(
            try store.createNode(
                id: n2,
                nodeType: .text,
                parentID: n1,
                properties: [(.text, .string("Oversized string payload"))]
            )
        ) { error in
            guard case StoreError.maxStringLengthExceeded = error else {
                XCTFail("Expected maxStringLengthExceeded, got \(error)")
                return
            }
        }
        XCTAssertFalse(store.containsNode(n2))
        XCTAssertFalse(store.isIDUsed(n2))
    }

    func testNestedValueDepthLimitEnforced() throws {
        let limits = StoreLimits(
            maxTreeDepth: 64,
            maxNodeCount: 1000,
            maxStringLength: 1024,
            maxValueDepth: 3,
            maxListElements: 100,
            maxRecordProperties: 100
        )
        var store = SemanticStore(limits: limits)

        // Depth 1: List containing scalar
        let valDepth1: Value = .list([.signedInt(42)])
        try store.createNode(
            id: NodeId(1),
            nodeType: .surface,
            properties: [(.value, valDepth1)]
        )

        // Depth 4: List -> List -> List -> scalar (depth 4 when inspecting inner)
        let nestedVal: Value = .list([.list([.list([.list([.signedInt(1)])])])])

        XCTAssertThrowsError(try store.setProperty(nodeID: NodeId(1), property: .value, value: nestedVal)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxValueDepthExceeded(limit: 3, actual: 4))
        }
    }

    func testMaxListElementsLimitEnforced() throws {
        let limits = StoreLimits(
            maxTreeDepth: 64,
            maxNodeCount: 1000,
            maxStringLength: 1024,
            maxValueDepth: 10,
            maxListElements: 3,
            maxRecordProperties: 100
        )
        var store = SemanticStore(limits: limits)

        let listOk: Value = .list([.signedInt(1), .signedInt(2), .signedInt(3)])
        try store.createNode(
            id: NodeId(1),
            nodeType: .surface,
            properties: [(.items, listOk)]
        )

        let listTooLong: Value = .list([.signedInt(1), .signedInt(2), .signedInt(3), .signedInt(4)])
        XCTAssertThrowsError(try store.setProperty(nodeID: NodeId(1), property: .items, value: listTooLong)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxListLengthExceeded(limit: 3, actual: 4))
        }
    }

    func testMaxRecordPropertiesLimitEnforced() {
        let limits = StoreLimits(
            maxTreeDepth: 64,
            maxNodeCount: 1000,
            maxStringLength: 1024,
            maxValueDepth: 10,
            maxListElements: 100,
            maxRecordProperties: 2
        )
        var store = SemanticStore(limits: limits)

        let recordTooManyProps: Value = .record(SmallRecord(
            typeRef: .standard(1),
            properties: [
                Property(property: .standard(1), value: .string("A")),
                Property(property: .standard(2), value: .string("B")),
                Property(property: .standard(3), value: .string("C"))
            ]
        ))

        XCTAssertThrowsError(
            try store.createNode(
                id: NodeId(1),
                nodeType: .surface,
                properties: [(.value, recordTooManyProps)]
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxRecordPropertiesExceeded(limit: 2, actual: 3))
        }
    }

    // MARK: - Protobuf Wire Operations (Mirroring Task 4)

    func testApplyProtobufWireOperations() throws {
        var store = SemanticStore()

        // 1. CreateNodeOp
        var createOp = SRUIOperation()
        var nodeRecord = Srui_Protocol_NodeRecord()
        nodeRecord.nodeID = 100
        nodeRecord.type = TypeRef.surface.toWire()
        nodeRecord.parentID = 0
        nodeRecord.childIndex = 0
        var prop1 = Srui_Protocol_Property()
        prop1.property = PropertyRef.label.toWire()
        prop1.value = Value.string("Wire Surface").toWire()
        nodeRecord.properties = [prop1]
        var createNodePayload = Srui_Protocol_CreateNodeOp()
        createNodePayload.node = nodeRecord
        createOp.createNode = createNodePayload

        try store.apply(wire: createOp)
        XCTAssertEqual(store.nodeCount, 1)

        // 2. SetPropertyOp
        var setOp = SRUIOperation()
        var setPayload = Srui_Protocol_SetPropertyOp()
        setPayload.nodeID = 100
        setPayload.property = PropertyRef.enabled.toWire()
        setPayload.value = Value.bool(true).toWire()
        setOp.setProperty = setPayload

        try store.apply(wire: setOp)
        XCTAssertEqual(store.getNode(NodeId(100))?.getProperty(.enabled), .bool(true))

        // 3. ClearPropertyOp
        var clearOp = SRUIOperation()
        var clearPayload = Srui_Protocol_ClearPropertyOp()
        clearPayload.nodeID = 100
        clearPayload.property = PropertyRef.enabled.toWire()
        clearOp.clearProperty_p = clearPayload

        try store.apply(wire: clearOp)
        XCTAssertNil(store.getNode(NodeId(100))?.getProperty(.enabled))

        // 4. DeleteNodeOp
        var delOp = SRUIOperation()
        var delPayload = Srui_Protocol_DeleteNodeOp()
        delPayload.nodeID = 100
        delOp.deleteNode = delPayload

        try store.apply(wire: delOp)
        XCTAssertTrue(store.isEmpty)
        XCTAssertTrue(store.isIDUsed(NodeId(100)))

        // 5. Create children with exact index vs append sentinel (UInt32.max)
        var rootOp = SRUIOperation()
        var rootRecord = Srui_Protocol_NodeRecord()
        rootRecord.nodeID = 1
        rootRecord.type = TypeRef.surface.toWire()
        var rootPayload = Srui_Protocol_CreateNodeOp()
        rootPayload.node = rootRecord
        rootOp.createNode = rootPayload
        try store.apply(wire: rootOp)

        // Child 1 at index 0
        var child1Op = SRUIOperation()
        var c1Record = Srui_Protocol_NodeRecord()
        c1Record.nodeID = 2
        c1Record.type = TypeRef.button.toWire()
        c1Record.parentID = 1
        c1Record.childIndex = 0
        var c1Payload = Srui_Protocol_CreateNodeOp()
        c1Payload.node = c1Record
        child1Op.createNode = c1Payload
        try store.apply(wire: child1Op)

        // Child 2 appended using UInt32.max sentinel
        var child2Op = SRUIOperation()
        var c2Record = Srui_Protocol_NodeRecord()
        c2Record.nodeID = 3
        c2Record.type = TypeRef.text.toWire()
        c2Record.parentID = 1
        c2Record.childIndex = UInt32.max
        var c2Payload = Srui_Protocol_CreateNodeOp()
        c2Payload.node = c2Record
        child2Op.createNode = c2Payload
        try store.apply(wire: child2Op)

        XCTAssertEqual(store.children(of: NodeId(1)), [NodeId(2), NodeId(3)])

        // Child 3 inserted at index 1 (between 2 and 3)
        var child3Op = SRUIOperation()
        var c3Record = Srui_Protocol_NodeRecord()
        c3Record.nodeID = 4
        c3Record.type = TypeRef.toggle.toWire()
        c3Record.parentID = 1
        c3Record.childIndex = 1
        var c3Payload = Srui_Protocol_CreateNodeOp()
        c3Payload.node = c3Record
        child3Op.createNode = c3Payload
        try store.apply(wire: child3Op)

        XCTAssertEqual(store.children(of: NodeId(1)), [NodeId(2), NodeId(4), NodeId(3)])

        // Move child 2 to end using append sentinel
        var moveOp = SRUIOperation()
        var movePayload = Srui_Protocol_MoveNodeOp()
        movePayload.nodeID = 2
        movePayload.newParentID = 1
        movePayload.newChildIndex = UInt32.max
        moveOp.moveNode = movePayload
        try store.apply(wire: moveOp)

        XCTAssertEqual(store.children(of: NodeId(1)), [NodeId(4), NodeId(3), NodeId(2)])
    }

    func testWireCreateRootNodesPreserveExplicitChildIndex() throws {
        var store = SemanticStore()

        // Second root inserted at index 0 (prepend)
        var rootB = SRUIOperation()
        var recB = Srui_Protocol_NodeRecord()
        recB.nodeID = 2
        recB.type = TypeRef.surface.toWire()
        recB.childIndex = 0
        var payB = Srui_Protocol_CreateNodeOp()
        payB.node = recB
        rootB.createNode = payB
        try store.apply(wire: rootB)

        // First root appended at index 1
        var rootA = SRUIOperation()
        var recA = Srui_Protocol_NodeRecord()
        recA.nodeID = 1
        recA.type = TypeRef.surface.toWire()
        recA.childIndex = 1
        var payA = Srui_Protocol_CreateNodeOp()
        payA.node = recA
        rootA.createNode = payA
        try store.apply(wire: rootA)

        XCTAssertEqual(store.rootIDs, [NodeId(2), NodeId(1)], "Wire root childIndex must be preserved for multi-root ordering")
    }

    // MARK: - Atomic Batch Execution & Rollback Invariant (§12.1)

    func testAtomicBatchApplicationWithInvalidOpLeavesStoreUnchanged() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let colID = NodeId(2)

        // Baseline pre-call state: 2 nodes
        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: colID, nodeType: .column, parentID: rootID)

        XCTAssertEqual(store.nodeCount, 2)
        XCTAssertEqual(store.children(of: colID), [])

        // Prepare a batch of operations with an invalid operation partway through:
        // 1. Create Text (#3) (valid)
        // 2. Create Button (#4) (valid)
        // 3. SetProperty on Column (#2) (valid)
        // 4. INVALID: Create node under nonexistent parent #999
        // 5. Create Toggle (#5) (valid)
        let operations: [StoreOperation] = [
            .create(id: NodeId(3), nodeType: .text, parentID: colID, properties: [(.text, .string("Child 1"))]),
            .create(id: NodeId(4), nodeType: .button, parentID: colID, properties: [(.label, .string("Child 2"))]),
            .setProperty(id: colID, property: .enabled, value: .bool(false)),
            .create(id: NodeId(100), nodeType: .text, parentID: NodeId(999), properties: []),
            .create(id: NodeId(5), nodeType: .toggle, parentID: colID, properties: [])
        ]

        // Execute atomic batch
        XCTAssertThrowsError(try store.apply(operations)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.parentNotFound(NodeId(999)))
        }

        // ASSERTION: Store is left EXACTLY in its pre-call state!
        // No operation before the failure point is left "half visible".
        XCTAssertEqual(store.nodeCount, 2, "Node count must remain at pre-call state (2)")
        XCTAssertTrue(store.containsNode(rootID))
        XCTAssertTrue(store.containsNode(colID))
        XCTAssertFalse(store.containsNode(NodeId(3)), "Node #3 from aborted batch must not exist")
        XCTAssertFalse(store.containsNode(NodeId(4)), "Node #4 from aborted batch must not exist")
        XCTAssertFalse(store.containsNode(NodeId(100)), "Node #100 from aborted batch must not exist")
        XCTAssertFalse(store.containsNode(NodeId(5)), "Node #5 from aborted batch must not exist")

        // Uncommitted IDs must NOT be marked as used in the session
        XCTAssertFalse(store.isIDUsed(NodeId(3)))
        XCTAssertFalse(store.isIDUsed(NodeId(4)))
        XCTAssertFalse(store.isIDUsed(NodeId(100)))
        XCTAssertFalse(store.isIDUsed(NodeId(5)))

        // Column children and property state must be untouched
        XCTAssertEqual(store.children(of: colID), [])
        XCTAssertNil(store.getNode(colID)?.getProperty(.enabled), "Property set on colID must be rolled back")
    }

    func testAtomicBatchApplicationSuccess() throws {
        var store = SemanticStore()
        let rootID = NodeId(1)
        let colID = NodeId(2)

        try store.createNode(id: rootID, nodeType: .surface)
        try store.createNode(id: colID, nodeType: .column, parentID: rootID)

        let operations: [StoreOperation] = [
            .create(id: NodeId(3), nodeType: .text, parentID: colID, properties: [(.text, .string("Child 1"))]),
            .create(id: NodeId(4), nodeType: .button, parentID: colID, properties: [(.label, .string("Child 2"))]),
            .setProperty(id: colID, property: .enabled, value: .bool(false))
        ]

        try store.apply(operations)

        XCTAssertEqual(store.nodeCount, 4)
        XCTAssertEqual(store.children(of: colID), [NodeId(3), NodeId(4)])
        XCTAssertEqual(store.getNode(colID)?.getProperty(.enabled), .bool(false))
        XCTAssertTrue(store.isIDUsed(NodeId(3)))
        XCTAssertTrue(store.isIDUsed(NodeId(4)))
    }
}
