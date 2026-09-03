//
// TransactionTests.swift
// SemanticModelTests
//
// Integration tests for SemanticStore transactions and atomic revisions (§12, §12.1, §12.2, §26).
//

import XCTest
import Foundation
@testable import Protocol
@testable import SemanticModel

final class TransactionTests: XCTestCase {

    func testValidTransactionCommitsAndAdvancesRevisionByOne() throws {
        let store = SemanticStore()
        XCTAssertEqual(store.revision, Revision.initial)
        XCTAssertEqual(store.nodeCount, 0)

        let applier = TransactionApplier(store: store)

        // 1. Transaction 1 (base = 0 -> new = 1): Create root Surface and Column layout
        let rootID = NodeId(1)
        let colID = NodeId(2)
        let btnID = NodeId(3)

        let ops1: [StoreOperation] = [
            .createNode(
                id: rootID,
                nodeType: .surface,
                parentID: nil,
                childIndex: nil,
                properties: [Property(property: .label, value: .string("Main Window"))]
            ),
            .createNode(
                id: colID,
                nodeType: .column,
                parentID: rootID,
                childIndex: nil,
                properties: [Property(property: .spacingRole, value: .unsignedInt(1))]
            ),
            .createNode(
                id: btnID,
                nodeType: .button,
                parentID: colID,
                childIndex: nil,
                properties: [
                    Property(property: .label, value: .string("Submit")),
                    Property(property: .enabled, value: .bool(true))
                ]
            )
        ]

        let res1 = applier.apply(baseRevision: .initial, operations: ops1)
        guard case .success(let newRev1) = res1 else {
            XCTFail("Transaction 1 should commit cleanly: \(res1)")
            return
        }

        XCTAssertEqual(newRev1, Revision(1))
        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 3)
        XCTAssertEqual(applier.store.rootIDs, [rootID])
        XCTAssertEqual(applier.store.children(of: rootID), [colID])
        XCTAssertEqual(applier.store.children(of: colID), [btnID])

        let btnNode = try XCTUnwrap(applier.store.getNode(btnID))
        XCTAssertEqual(btnNode.getProperty(.label), .string("Submit"))
        XCTAssertEqual(btnNode.getProperty(.enabled), .bool(true))

        // 2. Transaction 2 (base = 1 -> new = 2): Mutate properties and add a Text node
        let textID = NodeId(4)
        let ops2: [StoreOperation] = [
            .setProperty(id: btnID, property: .enabled, value: .bool(false)),
            .createNode(
                id: textID,
                nodeType: .text,
                parentID: colID,
                childIndex: 0, // insert at beginning of column
                properties: [Property(property: .text, value: .string("Status: Processing"))]
            )
        ]

        let res2 = applier.apply(baseRevision: Revision(1), operations: ops2)
        guard case .success(let newRev2) = res2 else {
            XCTFail("Transaction 2 should commit cleanly: \(res2)")
            return
        }

        XCTAssertEqual(newRev2, Revision(2))
        XCTAssertEqual(applier.store.revision, Revision(2))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(2))
        XCTAssertEqual(applier.store.nodeCount, 4)
        XCTAssertEqual(applier.store.children(of: colID), [textID, btnID])

        let updatedBtn = try XCTUnwrap(applier.store.getNode(btnID))
        XCTAssertEqual(updatedBtn.getProperty(.enabled), .bool(false))
    }

    func testTransactionWithInvalidLastOpAbortsWithZeroSideEffects() throws {
        let applier = TransactionApplier()

        // Setup initial valid state at Revision 1
        let rootID = NodeId(1)
        let setupRes = applier.apply(
            baseRevision: .initial,
            operations: [.createNode(
                id: rootID,
                nodeType: .surface,
                parentID: nil,
                childIndex: nil,
                properties: [Property(property: .label, value: .string("Initial Title"))]
            )]
        )
        XCTAssertEqual(setupRes, .success(Revision(1)))
        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 1)

        // Transaction with 3 ops where op 1 and op 2 are valid, but op 3 fails on nonexistent node
        let colID = NodeId(2)
        let nonexistent = NodeId(999)

        let failingOps: [StoreOperation] = [
            .createNode(id: colID, nodeType: .column, parentID: rootID),
            .setProperty(id: rootID, property: .label, value: .string("Modified Title")),
            .setProperty(id: nonexistent, property: .text, value: .string("Fail here"))
        ]

        let failRes = applier.apply(baseRevision: Revision(1), operations: failingOps)
        XCTAssertEqual(
            failRes,
            .failure(.opFailed(opIndex: 2, source: .nodeNotFound(nonexistent)))
        )

        // Verify ZERO visible side effects and unadvanced revision (§12.1)
        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 1)
        XCTAssertFalse(applier.store.containsNode(colID))
        XCTAssertFalse(applier.store.isIDUsed(colID)) // NodeId(2) must not be consumed by aborted txn
        XCTAssertEqual(applier.store.children(of: rootID), [])

        let rootNode = try XCTUnwrap(applier.store.getNode(rootID))
        XCTAssertEqual(rootNode.getProperty(.label), .string("Initial Title"))

        // Verify that a subsequent valid transaction using NodeId(2) succeeds completely
        let recoveryOps: [StoreOperation] = [
            .createNode(id: colID, nodeType: .column, parentID: rootID)
        ]
        let newRev = applier.apply(baseRevision: Revision(1), operations: recoveryOps)
        XCTAssertEqual(newRev, .success(Revision(2)))
        XCTAssertEqual(applier.store.revision, Revision(2))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(2))
        XCTAssertEqual(applier.store.nodeCount, 2)
        XCTAssertTrue(applier.store.containsNode(colID))
    }

    func testTransactionWithStaleOrWrongBaseRevisionRejected() {
        let applier = TransactionApplier()

        // Commit transaction 1
        _ = applier.apply(
            baseRevision: .initial,
            operations: [.createNode(id: NodeId(1), nodeType: .surface)]
        )
        XCTAssertEqual(applier.store.revision, Revision(1))

        // 1. Submit with stale baseRevision (0 when current is 1)
        let staleRes = applier.apply(
            baseRevision: Revision(0),
            operations: [.createNode(id: NodeId(2), nodeType: .button, parentID: NodeId(1))]
        )
        XCTAssertEqual(
            staleRes,
            .failure(.staleBaseRevision(expected: Revision(1), actual: Revision(0)))
        )

        // 2. Submit with future / skipped baseRevision (5 when current is 1)
        let futureRes = applier.apply(
            baseRevision: Revision(5),
            operations: [.createNode(id: NodeId(2), nodeType: .button, parentID: NodeId(1))]
        )
        XCTAssertEqual(
            futureRes,
            .failure(.staleBaseRevision(expected: Revision(1), actual: Revision(5)))
        )

        // Store state and revision remain unchanged
        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 1)
        XCTAssertFalse(applier.store.containsNode(NodeId(2)))
    }

    func testMaxOperationsLimitEnforcedAsPrecheck() {
        // Configure store with maxTransactionOperations = 3
        let limits = StoreLimits().withMaxTransactionOperations(3)
        let applier = TransactionApplier(limits: limits)

        // Submit transaction with 4 operations (exceeds limit of 3)
        let oversizedOps: [StoreOperation] = [
            .createNode(id: NodeId(1), nodeType: .surface),
            .createNode(id: NodeId(2), nodeType: .column, parentID: NodeId(1)),
            .createNode(id: NodeId(3), nodeType: .button, parentID: NodeId(2)),
            .createNode(id: NodeId(4), nodeType: .text, parentID: NodeId(2))
        ]

        let res = applier.apply(baseRevision: .initial, operations: oversizedOps)
        XCTAssertEqual(
            res,
            .failure(.maxOperationsExceeded(limit: 3, actual: 4))
        )

        // Pre-check prevents any execution: store is completely untouched
        XCTAssertEqual(applier.store.revision, Revision.initial)
        XCTAssertTrue(applier.store.isEmpty)
        XCTAssertFalse(applier.store.isIDUsed(NodeId(1)))
        XCTAssertFalse(applier.store.isIDUsed(NodeId(2)))
    }

    func testForwardCoalescedTransactionCommitsAndAdvancesRevision() {
        let applier = TransactionApplier()

        let rootID = NodeId(1)
        let setupRes = applier.apply(baseRevision: Revision(0), operations: [.createNode(id: rootID, nodeType: .surface)])
        XCTAssertEqual(setupRes, .success(Revision(1)))

        let scalarOps: [StoreOperation] = [.setProperty(id: rootID, property: .label, value: .string("v5"))]

        // Coalesced delta specifies forward range base 1 -> new 5 (§12.1 delivery forms, §20.4)
        let forwardTxn = Transaction(baseRevision: Revision(1), newRevision: Revision(5), operations: scalarOps, priority: 0)

        // The authoritative path advances exactly one revision and refuses the span
        XCTAssertEqual(
            applier.applyCommitted(record: forwardTxn).map(\.revision),
            .failure(.invalidNewRevision(expected: Revision(2), actual: Revision(5)))
        )
        XCTAssertEqual(applier.store.revision, Revision(1), "a refused commit must not advance the replica")

        // The delivery path accepts it, because a replica is what a delta is addressed to
        let res = applier.applyDelivered(record: forwardTxn)
        guard case .success(let snapshot) = res else {
            XCTFail("expected success for forward coalesced delta, got \(res)")
            return
        }
        XCTAssertEqual(snapshot.revision, Revision(5))
        XCTAssertEqual(applier.store.revision, Revision(5))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(5))
        XCTAssertEqual(applier.store.nodeCount, 1)

        // Forward range with structural mutation (non-coalesceable) is rejected on both paths
        // (§12.1, §20.4)
        let structuralSpanTxn = Transaction(
            baseRevision: Revision(5),
            newRevision: Revision(10),
            operations: [.deleteNode(id: rootID)],
            priority: 0
        )
        XCTAssertEqual(
            applier.applyCommitted(record: structuralSpanTxn).map(\.revision),
            .failure(.invalidNewRevision(expected: Revision(6), actual: Revision(10)))
        )
        XCTAssertEqual(
            applier.applyDelivered(record: structuralSpanTxn).map(\.revision),
            .failure(.invalidNewRevision(expected: Revision(6), actual: Revision(10)))
        )

        // Equal revision is rejected by both paths
        let equalTxn = Transaction(baseRevision: Revision(5), newRevision: Revision(5), operations: [], priority: 0)
        XCTAssertEqual(
            applier.applyCommitted(record: equalTxn).map(\.revision),
            .failure(.invalidNewRevision(expected: Revision(6), actual: Revision(5)))
        )
        XCTAssertEqual(
            applier.applyDelivered(record: equalTxn).map(\.revision),
            .failure(.invalidNewRevision(expected: Revision(6), actual: Revision(5)))
        )

        // Backward revision is rejected by both paths
        let backwardTxn = Transaction(baseRevision: Revision(5), newRevision: Revision(3), operations: [], priority: 0)
        XCTAssertEqual(
            applier.applyCommitted(record: backwardTxn).map(\.revision),
            .failure(.invalidNewRevision(expected: Revision(6), actual: Revision(3)))
        )
        XCTAssertEqual(
            applier.applyDelivered(record: backwardTxn).map(\.revision),
            .failure(.invalidNewRevision(expected: Revision(6), actual: Revision(3)))
        )
    }

    func testTransactionRecordWithInvalidNewRevisionRejected() {
        let applier = TransactionApplier()

        let rootID = NodeId(1)
        let ops: [StoreOperation] = [.createNode(id: rootID, nodeType: .surface)]

        // Transaction specifies newRevision = 5 instead of expected 1 (§12.1)
        let invalidTxn = Transaction(baseRevision: Revision(0), newRevision: Revision(5), operations: ops, priority: 0)

        let res = applier.apply(record: invalidTxn)
        XCTAssertEqual(
            res,
            .failure(.invalidNewRevision(expected: Revision(1), actual: Revision(5)))
        )

        XCTAssertEqual(applier.store.revision, Revision.initial)
        XCTAssertTrue(applier.store.isEmpty)
    }

    func testWireTransactionConversionAndApplication() {
        let applier = TransactionApplier()

        let rootID = NodeId(10)
        let childID = NodeId(20)

        var wireTxn = SRUITransaction()
        wireTxn.baseRevision = 0
        wireTxn.newRevision = 1
        wireTxn.priority = 1

        var op1 = SRUIOperation()
        var create1 = Srui_Protocol_CreateNodeOp()
        var rec1 = Srui_Protocol_NodeRecord()
        rec1.nodeID = rootID.value
        rec1.type = TypeRef.surface.toWire()
        rec1.parentID = 0
        rec1.childIndex = 0
        var prop1 = Srui_Protocol_Property()
        prop1.property = PropertyRef.label.toWire()
        prop1.value = Value.string("Wire Surface").toWire()
        rec1.properties = [prop1]
        create1.node = rec1
        op1.createNode = create1

        var op2 = SRUIOperation()
        var create2 = Srui_Protocol_CreateNodeOp()
        var rec2 = Srui_Protocol_NodeRecord()
        rec2.nodeID = childID.value
        rec2.type = TypeRef.button.toWire()
        rec2.parentID = rootID.value
        rec2.childIndex = UInt32.max
        var prop2 = Srui_Protocol_Property()
        prop2.property = PropertyRef.label.toWire()
        prop2.value = Value.string("Wire Button").toWire()
        rec2.properties = [prop2]
        create2.node = rec2
        op2.createNode = create2

        wireTxn.operations = [op1, op2]

        let res = applier.apply(wire: wireTxn)
        guard case .success(let committedRev) = res else {
            XCTFail("Apply wire transaction failed: \(res)")
            return
        }

        XCTAssertEqual(committedRev, Revision(1))
        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 2)
        XCTAssertEqual(applier.store.children(of: rootID), [childID])
    }

    func testApplyWireTransactionRejectsOversizedOperationsWithoutStoreMutation() throws {
        var customLimits = StoreLimits()
        customLimits.maxTransactionOperations = 2

        var wireTxn = SRUITransaction()
        wireTxn.baseRevision = Revision.initial.value
        wireTxn.newRevision = Revision(1).value
        var ops: [SRUIOperation] = []
        for i in 1...3 {
            var op = SRUIOperation()
            var del = Srui_Protocol_DeleteNodeOp()
            del.nodeID = UInt64(i)
            op.deleteNode = del
            ops.append(op)
        }
        wireTxn.operations = ops

        let applier = TransactionApplier(limits: customLimits)
        let originalRevision = applier.store.revision
        let originalNodeCount = applier.store.nodeCount

        let res = applier.apply(wire: wireTxn)
        guard case .failure(.wireError(let message)) = res else {
            XCTFail("Expected wireError failure, got \(res)")
            return
        }
        XCTAssertTrue(message.contains("max limit of 2"))

        XCTAssertEqual(applier.store.revision, originalRevision)
        XCTAssertEqual(applier.store.nodeCount, originalNodeCount)
    }

    func testIntermediateFailureRollsBackEntireTransaction() {
        let applier = TransactionApplier()

        // Create root (#1), parent (#2), and row (#3)
        _ = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: NodeId(2), nodeType: .column, parentID: NodeId(1)),
                .createNode(id: NodeId(3), nodeType: .row, parentID: NodeId(2))
            ]
        )

        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 3)

        // Try a transaction with 4 ops:
        // Op 0: create #4 under #3 (valid)
        // Op 1: set_property on #1 (valid)
        // Op 2: move #2 under #3 -> CycleDetected error!
        // Op 3: create #5 (valid)
        let cycleOps: [StoreOperation] = [
            .createNode(id: NodeId(4), nodeType: .text, parentID: NodeId(3)),
            .setProperty(id: NodeId(1), property: .label, value: .string("Will Rollback")),
            .moveNode(id: NodeId(2), newParentID: NodeId(3), newChildIndex: nil),
            .createNode(id: NodeId(5), nodeType: .button, parentID: NodeId(3))
        ]

        let res = applier.apply(baseRevision: Revision(1), operations: cycleOps)
        XCTAssertEqual(
            res,
            .failure(.opFailed(
                opIndex: 2,
                source: .cycleDetected(nodeID: NodeId(2), targetParent: NodeId(3))
            ))
        )

        // Assert complete rollback
        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 3)
        XCTAssertFalse(applier.store.containsNode(NodeId(4)))
        XCTAssertFalse(applier.store.containsNode(NodeId(5)))
        XCTAssertFalse(applier.store.isIDUsed(NodeId(4)))
        XCTAssertFalse(applier.store.isIDUsed(NodeId(5)))
        XCTAssertEqual(applier.store.parent(of: NodeId(2)), .some(NodeId(1)))
        XCTAssertNil(applier.store.getNode(NodeId(1))?.getProperty(.label))
    }

    func testEmptyTransactionAdvancesRevisionByOne() {
        let applier = TransactionApplier()
        XCTAssertEqual(applier.store.revision, Revision.initial)

        let res1 = applier.apply(baseRevision: .initial, operations: [])
        XCTAssertEqual(res1, .success(Revision(1)))
        XCTAssertEqual(applier.store.revision, Revision(1))

        let res2 = applier.apply(baseRevision: Revision(1), operations: [])
        XCTAssertEqual(res2, .success(Revision(2)))
        XCTAssertEqual(applier.store.revision, Revision(2))
    }

    func testMonotonicSequentialRevisions() {
        let applier = TransactionApplier()

        // Apply 10 sequential transactions
        for i: UInt64 in 0..<10 {
            let base = Revision(i)
            let nodeID = NodeId(i + 1)
            let op: StoreOperation
            if i == 0 {
                op = .createNode(id: nodeID, nodeType: .surface)
            } else {
                op = .createNode(id: nodeID, nodeType: .text, parentID: NodeId(1))
            }

            let res = applier.apply(baseRevision: base, operations: [op])
            XCTAssertEqual(res, .success(Revision(i + 1)))
            XCTAssertEqual(applier.store.revision, Revision(i + 1))
        }

        XCTAssertEqual(applier.store.revision, Revision(10))
        XCTAssertEqual(applier.lastAppliedRevision, Revision(10))
        XCTAssertEqual(applier.store.nodeCount, 10)
    }

    /// A frame decoded from the wire may claim `baseRevision = UInt64.max`, a revision with no
    /// successor. Every validation path computes `baseRevision.next`, which traps on overflow, so
    /// an unguarded check turns a malformed server frame into a client crash (§12.1, §26).
    func testExhaustedBaseRevisionIsRejectedWithoutOverflowTrap() {
        let applier = TransactionApplier()
        let exhausted = Revision(UInt64.max)
        let txn = Transaction(baseRevision: exhausted, newRevision: Revision(0), operations: [], priority: 0)

        XCTAssertEqual(
            applier.applyCommitted(record: txn).map(\.revision),
            .failure(.revisionExhausted(base: exhausted)),
            "an exhausted base revision has no successor and cannot be an authoritative commit"
        )
        XCTAssertEqual(
            applier.applyDelivered(record: txn).map(\.revision),
            .failure(.revisionExhausted(base: exhausted)),
            "neither delivery form admits an exhausted base revision"
        )
        XCTAssertEqual(
            applier.apply(baseRevision: exhausted, operations: []),
            .failure(.staleBaseRevision(expected: .initial, actual: exhausted)),
            "an exhausted base revision does not match the store and must be refused"
        )
        XCTAssertEqual(applier.store.revision, .initial, "a refused frame must not advance the replica")
    }

    /// The wire helpers are the documented entry point for SDK consumers, so they must cover every
    /// form the server legitimately emits: a coalesced scalar delta spanning several revisions is
    /// refused by the authoritative helper and accepted by the delivered one (§12.1, §20.4).
    func testDeliveredWireTransactionAcceptsCoalescedScalarSpan() {
        let applier = TransactionApplier()
        let rootID = NodeId(1)
        XCTAssertEqual(
            applier.apply(baseRevision: .initial, operations: [.createNode(id: rootID, nodeType: .surface)]),
            .success(Revision(1))
        )

        var wireTxn = SRUITransaction()
        wireTxn.baseRevision = 1
        wireTxn.newRevision = 5
        var setOp = SRUIOperation()
        var setPayload = Srui_Protocol_SetPropertyOp()
        setPayload.nodeID = rootID.value
        setPayload.property = PropertyRef.label.toWire()
        setPayload.value = Value.string("v5").toWire()
        setOp.setProperty = setPayload
        wireTxn.operations = [setOp]

        XCTAssertEqual(
            applier.apply(wire: wireTxn),
            .failure(.invalidNewRevision(expected: Revision(2), actual: Revision(5))),
            "the authoritative wire helper must keep refusing a multi-revision span"
        )
        XCTAssertEqual(applier.store.revision, Revision(1))

        XCTAssertEqual(applier.applyDelivered(wire: wireTxn), .success(Revision(5)))
        XCTAssertEqual(applier.store.revision, Revision(5))
        XCTAssertEqual(applier.store.getNode(rootID)?.getProperty(.label), .string("v5"))

        var store = SemanticStore()
        XCTAssertEqual(
            store.applyWireTransaction(rootCreateWire(rootID)),
            .success(Revision(1))
        )
        XCTAssertEqual(store.applyDeliveredWireTransaction(wireTxn), .success(Revision(5)))
        XCTAssertEqual(store.revision, Revision(5))
        XCTAssertEqual(store.getNode(rootID)?.getProperty(.label), .string("v5"))
    }

    /// Builds a wire transaction creating `id` as a root Surface at revision 0 -> 1.
    private func rootCreateWire(_ id: NodeId) -> SRUITransaction {
        var wire = SRUITransaction()
        wire.baseRevision = 0
        wire.newRevision = 1
        var op = SRUIOperation()
        var payload = Srui_Protocol_CreateNodeOp()
        var record = Srui_Protocol_NodeRecord()
        record.nodeID = id.value
        record.type = TypeRef.surface.toWire()
        record.parentID = 0
        record.childIndex = 0
        payload.node = record
        op.createNode = payload
        wire.operations = [op]
        return wire
    }
}
