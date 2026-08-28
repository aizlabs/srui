//
// ModelTests.swift
// SemanticModelTests
//
// Integration tests for collection models, sparse caching, and model mutations (§8, §13).
//

import XCTest
import Foundation
@testable import Protocol
@testable import SemanticModel

final class ModelTests: XCTestCase {

    func testCreateSparseModelAndItemMutationsByIdentity() throws {
        var store = SemanticStore()

        let tableType = try XCTUnwrap(resolveStandardNodeType("Table").get())
        let modelID = ModelId(7)
        let largeCount: UInt64 = 500_000

        // 1. Create a model with large itemCount (500,000) and only a few cached items (§8)
        try store.createModel(id: modelID, modelType: tableType, itemCount: largeCount)

        XCTAssertEqual(store.modelCount, 1)
        let model = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(model.itemCount, largeCount)
        XCTAssertEqual(model.cachedItemCount, 0)
        XCTAssertTrue(model.cachedRanges().isEmpty)

        // 2. Insert initial items at specific sparse positions
        let colName = PropertyRef.standard(1)
        let colCpu = PropertyRef.standard(2)

        let item1 = ModelItem(
            itemID: ItemId(101),
            value: .string("nginx"),
            properties: [(colName, .string("nginx")), (colCpu, .float64(0.05))]
        )
        let item2 = ModelItem(
            itemID: ItemId(202),
            value: .string("postgres"),
            properties: [(colName, .string("postgres")), (colCpu, .float64(0.35))]
        )
        let item3 = ModelItem(
            itemID: ItemId(303),
            value: .string("redis"),
            properties: [(colName, .string("redis")), (colCpu, .float64(0.02))]
        )

        // Reset range at index 10 for item1 and item2
        try store.modelResetRange(id: modelID, startIndex: 10, items: [item1, item2], totalCount: nil)

        // Reset range at index 1000 for item3
        try store.modelResetRange(id: modelID, startIndex: 1000, items: [item3], totalCount: nil)

        let modelAfterReset = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(modelAfterReset.itemCount, largeCount)
        XCTAssertEqual(modelAfterReset.cachedItemCount, 3)
        XCTAssertEqual(modelAfterReset.cachedRanges().count, 2)
        XCTAssertEqual(modelAfterReset.getItemById(ItemId(101))?.value, .string("nginx"))
        XCTAssertEqual(modelAfterReset.getItemById(ItemId(202))?.value, .string("postgres"))
        XCTAssertEqual(modelAfterReset.getItemById(ItemId(303))?.value, .string("redis"))

        // 3. Insert a new item into the sparse collection using MODEL_INSERT (§13)
        let itemInserted = ModelItem(
            itemID: ItemId(150),
            value: .string("memcached"),
            properties: [(colName, .string("memcached")), (colCpu, .float64(0.01))]
        )
        try store.modelInsert(id: modelID, index: 11, items: [itemInserted])

        // Verify insertion: item count incremented, item1 at index 10 untouched, item2 shifted to 12, item3 shifted to 1001
        let modelAfterInsert = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(modelAfterInsert.itemCount, largeCount + 1)
        XCTAssertEqual(modelAfterInsert.cachedItemCount, 4)
        XCTAssertEqual(modelAfterInsert.indexOf(ItemId(101)), 10)
        XCTAssertEqual(modelAfterInsert.indexOf(ItemId(150)), 11)
        XCTAssertEqual(modelAfterInsert.indexOf(ItemId(202)), 12)
        XCTAssertEqual(modelAfterInsert.indexOf(ItemId(303)), 1001)

        // 4. Update an addressed item by itemID (update postgres -> postgres-master)
        let updatedPostgres = ModelItem(
            itemID: ItemId(202),
            value: .string("postgres-master"),
            properties: [(colName, .string("postgres-master")), (colCpu, .float64(0.50))]
        )
        try store.modelUpdate(id: modelID, index: nil, items: [updatedPostgres])

        // Confirm only the addressed item changes; other items remain untouched
        let modelAfterUpdate = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(modelAfterUpdate.getItemById(ItemId(202))?.value, .string("postgres-master"))
        XCTAssertEqual(modelAfterUpdate.getItemById(ItemId(202))?.getProperty(colCpu), .float64(0.50))
        XCTAssertEqual(modelAfterUpdate.getItemById(ItemId(101))?.value, .string("nginx"))
        XCTAssertEqual(modelAfterUpdate.getItemById(ItemId(150))?.value, .string("memcached"))
        XCTAssertEqual(modelAfterUpdate.getItemById(ItemId(303))?.value, .string("redis"))
        XCTAssertEqual(modelAfterUpdate.cachedItemCount, 4)

        // 5. Delete an item by itemID (delete nginx ItemId(101))
        try store.modelDelete(id: modelID, index: nil, count: nil, itemIds: [ItemId(101)])

        // Confirm only the addressed item was removed; others remain intact
        let modelAfterDelete = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(modelAfterDelete.cachedItemCount, 3)
        XCTAssertFalse(modelAfterDelete.containsItem(ItemId(101)))
        XCTAssertEqual(modelAfterDelete.getItemById(ItemId(150))?.value, .string("memcached"))
        XCTAssertEqual(modelAfterDelete.getItemById(ItemId(202))?.value, .string("postgres-master"))
        XCTAssertEqual(modelAfterDelete.getItemById(ItemId(303))?.value, .string("redis"))
    }

    func testModelResetRangeReplacesRangeWithoutTouchingCountOrOtherRanges() throws {
        var store = SemanticStore()

        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(42)
        let totalCount: UInt64 = 100_000

        try store.createModel(id: modelID, modelType: listType, itemCount: totalCount)

        // Populate Range A at index 100..103
        let rangeA = [
            ModelItem(itemID: ItemId(1), value: .string("A0")),
            ModelItem(itemID: ItemId(2), value: .string("A1")),
            ModelItem(itemID: ItemId(3), value: .string("A2"))
        ]
        try store.modelResetRange(id: modelID, startIndex: 100, items: rangeA, totalCount: nil)

        // Populate Range B at index 500..502
        let rangeB = [
            ModelItem(itemID: ItemId(10), value: .string("B0")),
            ModelItem(itemID: ItemId(11), value: .string("B1"))
        ]
        try store.modelResetRange(id: modelID, startIndex: 500, items: rangeB, totalCount: nil)

        let model = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(model.itemCount, 100_000)
        XCTAssertEqual(model.cachedItemCount, 5)
        XCTAssertEqual(model.cachedRanges().count, 2)

        // Reset Range A with new items (X0, X1, X2) without touching totalCount or Range B
        let newRangeA = [
            ModelItem(itemID: ItemId(1001), value: .string("X0")),
            ModelItem(itemID: ItemId(1002), value: .string("X1")),
            ModelItem(itemID: ItemId(1003), value: .string("X2"))
        ]
        try store.modelResetRange(id: modelID, startIndex: 100, items: newRangeA, totalCount: nil)

        let modelAfter = try XCTUnwrap(store.getModel(modelID))
        // 1. Total itemCount must NOT change
        XCTAssertEqual(modelAfter.itemCount, 100_000)

        // 2. Range A items are replaced
        XCTAssertFalse(modelAfter.containsItem(ItemId(1)))
        XCTAssertFalse(modelAfter.containsItem(ItemId(2)))
        XCTAssertFalse(modelAfter.containsItem(ItemId(3)))
        XCTAssertEqual(modelAfter.getItemById(ItemId(1001))?.value, .string("X0"))
        XCTAssertEqual(modelAfter.getItemById(ItemId(1002))?.value, .string("X1"))
        XCTAssertEqual(modelAfter.getItemById(ItemId(1003))?.value, .string("X2"))

        // 3. Range B items are completely unchanged
        XCTAssertEqual(modelAfter.getItemById(ItemId(10))?.value, .string("B0"))
        XCTAssertEqual(modelAfter.getItemById(ItemId(11))?.value, .string("B1"))
        XCTAssertEqual(modelAfter.indexOf(ItemId(10)), 500)
        XCTAssertEqual(modelAfter.indexOf(ItemId(11)), 501)
        XCTAssertEqual(modelAfter.cachedItemCount, 5)
    }

    func testTransactionAtomicitySpansNodeOpsAndModelOps() throws {
        let applier = TransactionApplier()

        let surfaceType = try XCTUnwrap(resolveStandardNodeType("Surface").get())
        let tableType = try XCTUnwrap(resolveStandardNodeType("Table").get())

        // Initialize baseline revision 1 with a surface and a model
        let rev1Res = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(id: NodeId(1), nodeType: surfaceType),
                .createModel(id: ModelId(10), modelType: tableType, itemCount: 1_000)
            ]
        )
        guard case .success(let rev1) = rev1Res else {
            XCTFail("Initial setup transaction failed: \(rev1Res)")
            return
        }
        XCTAssertEqual(rev1, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 1)
        XCTAssertEqual(applier.store.modelCount, 1)
        XCTAssertEqual(applier.store.getModel(ModelId(10))?.cachedItemCount, 0)

        // Build a transaction combining:
        // 1. A valid MODEL_INSERT on model 10
        // 2. An invalid CREATE_NODE op (referencing non-existent parent NodeId(999))
        let invalidTxnOps: [StoreOperation] = [
            .modelInsert(
                id: ModelId(10),
                index: 0,
                items: [
                    ModelItem(itemID: ItemId(50), value: .string("item-50")),
                    ModelItem(itemID: ItemId(51), value: .string("item-51"))
                ]
            ),
            // Invalid op: parent 999 does not exist!
            .createNode(id: NodeId(2), nodeType: surfaceType, parentID: NodeId(999))
        ]

        let result = applier.apply(baseRevision: rev1, operations: invalidTxnOps)
        XCTAssertEqual(
            result,
            .failure(.opFailed(opIndex: 1, source: .parentNotFound(NodeId(999))))
        )

        // Verify complete atomic rollback:
        // - Revision remains at rev1 (1)
        // - Node graph remains unchanged (1 node)
        // - Model 10 has 0 cached items (model_insert rolled back cleanly!)
        XCTAssertEqual(applier.store.revision, rev1)
        XCTAssertEqual(applier.store.nodeCount, 1)
        XCTAssertEqual(applier.store.modelCount, 1)
        let model = try XCTUnwrap(applier.store.getModel(ModelId(10)))
        XCTAssertEqual(model.cachedItemCount, 0)
        XCTAssertEqual(model.itemCount, 1_000)
        XCTAssertFalse(model.containsItem(ItemId(50)))
        XCTAssertFalse(model.containsItem(ItemId(51)))
    }

    func testNodeReferencingModelViaModelRefProperty() throws {
        var store = SemanticStore()

        let tableType = try XCTUnwrap(resolveStandardNodeType("Table").get())
        let modelRefProp = try XCTUnwrap(resolveStandardProperty("model_ref").get())
        let modelID = ModelId(77)

        // Create table node referencing Model #77 (§8)
        try store.createModel(id: modelID, modelType: tableType, itemCount: 50_000)
        try store.createNode(
            id: NodeId(40),
            nodeType: tableType,
            parentID: nil,
            childIndex: nil,
            properties: [(modelRefProp, .unsignedInt(77))]
        )

        let tableNode = try XCTUnwrap(store.getNode(NodeId(40)))
        XCTAssertEqual(tableNode.modelRef, modelID)

        let referencedModel = try XCTUnwrap(store.getModelForNode(NodeId(40)))
        XCTAssertEqual(referencedModel.id, modelID)
        XCTAssertEqual(referencedModel.itemCount, 50_000)
    }

    func testModelIdNeverReusedInSession() throws {
        var store = SemanticStore()
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(5)

        try store.createModel(id: modelID, modelType: listType, itemCount: 100)
        XCTAssertTrue(store.containsModel(modelID))

        // Delete model
        try store.deleteModel(modelID)
        XCTAssertFalse(store.containsModel(modelID))
        XCTAssertTrue(store.isModelIDUsed(modelID))

        // Attempting to recreate using same ModelId must fail (§6.2, §8)
        XCTAssertThrowsError(try store.createModel(id: modelID, modelType: listType, itemCount: 200)) { error in
            XCTAssertEqual(error as? StoreError, StoreError.modelIdAlreadyUsed(modelID))
        }
    }

    func testModelOperationsWireProtobufRoundtrip() throws {
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(99)

        let createOp: StoreOperation = .createModel(id: modelID, modelType: listType, itemCount: 10_000)
        let wireCreate = createOp.toWire()
        let roundtripCreate = try StoreOperation(wire: wireCreate)
        XCTAssertEqual(createOp, roundtripCreate)

        let insertOp: StoreOperation = .modelInsert(
            id: modelID,
            index: 5,
            items: [
                ModelItem(itemID: ItemId(1), value: .string("item1")),
                ModelItem(itemID: ItemId(2), value: .string("item2"))
            ]
        )
        let wireInsert = insertOp.toWire()
        let roundtripInsert = try StoreOperation(wire: wireInsert)
        XCTAssertEqual(insertOp, roundtripInsert)

        let deleteOp: StoreOperation = .modelDeleteItems(id: modelID, itemIds: [ItemId(1), ItemId(2)])
        let wireDelete = deleteOp.toWire()
        let roundtripDelete = try StoreOperation(wire: wireDelete)
        XCTAssertEqual(deleteOp, roundtripDelete)

        let updateOp: StoreOperation = .modelUpdate(
            id: modelID,
            index: 5,
            items: [ModelItem(itemID: ItemId(1), value: .string("item1-updated"))]
        )
        let wireUpdate = updateOp.toWire()
        let roundtripUpdate = try StoreOperation(wire: wireUpdate)
        XCTAssertEqual(updateOp, roundtripUpdate)

        let resetOp: StoreOperation = .modelResetRange(
            id: modelID,
            startIndex: 0,
            items: [ModelItem(itemID: ItemId(10), value: .string("r0"))],
            totalCount: 10_000
        )
        let wireReset = resetOp.toWire()
        let roundtripReset = try StoreOperation(wire: wireReset)
        XCTAssertEqual(resetOp, roundtripReset)
    }

    func testModelLimitsEnforcement() throws {
        let limits = StoreLimits()
        let maxStrLen = limits.maxStringLength
        var store = SemanticStore(limits: limits)

        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)
        try store.createModel(id: modelID, modelType: listType, itemCount: 100)

        // Create an item with an oversized string value
        let oversizedStr = String(repeating: "x", count: maxStrLen + 1)
        let invalidItem = ModelItem(itemID: ItemId(1), value: .string(oversizedStr))

        XCTAssertThrowsError(try store.modelInsert(id: modelID, index: 0, items: [invalidItem])) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxStringLengthExceeded(limit: maxStrLen, actual: maxStrLen + 1))
        }
    }

    func testSuccessfulMixedTransactionCommitsAtomically() throws {
        let applier = TransactionApplier()

        let surfaceType = try XCTUnwrap(resolveStandardNodeType("Surface").get())
        let treeType = try XCTUnwrap(resolveStandardNodeType("Tree").get())
        let modelRefProp = try XCTUnwrap(resolveStandardProperty("model_ref").get())

        let txn = Transaction(
            baseRevision: .initial,
            operations: [
                .createModel(id: ModelId(1), modelType: treeType, itemCount: 500),
                .modelResetRange(
                    id: ModelId(1),
                    startIndex: 0,
                    items: [
                        ModelItem(itemID: ItemId(10), value: .string("root_node")),
                        ModelItem(itemID: ItemId(11), value: .string("child_node"))
                    ],
                    totalCount: nil
                ),
                .createNode(
                    id: NodeId(1),
                    nodeType: surfaceType,
                    parentID: nil,
                    childIndex: nil,
                    properties: [Property(property: modelRefProp, value: .unsignedInt(1))]
                )
            ]
        )

        let newRev = applier.apply(record: txn)
        XCTAssertEqual(newRev, .success(Revision(1)))
        XCTAssertEqual(applier.store.revision, Revision(1))
        XCTAssertEqual(applier.store.nodeCount, 1)
        XCTAssertEqual(applier.store.modelCount, 1)

        let model = try XCTUnwrap(applier.store.getModel(ModelId(1)))
        XCTAssertEqual(model.cachedItemCount, 2)
        XCTAssertEqual(model.getItemById(ItemId(10))?.value, .string("root_node"))
    }

    func testResetRangeRejectsItemIdCachedOutsideReplacedRange() throws {
        var store = SemanticStore()
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)

        try store.createModel(id: modelID, modelType: listType, itemCount: 1_000)
        try store.modelResetRange(
            id: modelID,
            startIndex: 500,
            items: [ModelItem(itemID: ItemId(42), value: .string("cached-at-500"))],
            totalCount: nil
        )

        XCTAssertThrowsError(
            try store.modelResetRange(
                id: modelID,
                startIndex: 100,
                items: [ModelItem(itemID: ItemId(42), value: .string("collision"))],
                totalCount: nil
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.duplicateItemId(modelID: modelID, itemID: ItemId(42)))
        }
    }

    func testModelDeleteRangeRejectsOutOfBounds() throws {
        var store = SemanticStore()
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)

        try store.createModel(id: modelID, modelType: listType, itemCount: 10)

        XCTAssertThrowsError(
            try store.modelDelete(id: modelID, index: 5, count: 100, itemIds: [])
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.modelIndexOutOfBounds(index: 105, count: 10))
        }
    }

    func testModelDeleteRejectsCombinedIdentityAndRange() throws {
        var store = SemanticStore()
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)

        try store.createModel(id: modelID, modelType: listType, itemCount: 100)
        try store.modelResetRange(
            id: modelID,
            startIndex: 0,
            items: [
                ModelItem(itemID: ItemId(1), value: .string("a")),
                ModelItem(itemID: ItemId(2), value: .string("b"))
            ],
            totalCount: nil
        )

        XCTAssertThrowsError(
            try store.modelDelete(
                id: modelID,
                index: 0,
                count: 2,
                itemIds: [ItemId(1), ItemId(2)]
            )
        ) { error in
            guard case StoreError.invalidModelDelete = error else {
                XCTFail("Expected invalidModelDelete error, got \(error)")
                return
            }
        }
    }

    func testModelItemsPerOperationLimitEnforced() throws {
        let limits = StoreLimits().withMaxItemsPerModelOperation(2)
        var store = SemanticStore(limits: limits)
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)

        try store.createModel(id: modelID, modelType: listType, itemCount: 100)

        XCTAssertThrowsError(
            try store.modelInsert(
                id: modelID,
                index: 0,
                items: [
                    ModelItem(itemID: ItemId(1), value: .string("a")),
                    ModelItem(itemID: ItemId(2), value: .string("b")),
                    ModelItem(itemID: ItemId(3), value: .string("c"))
                ]
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxItemsPerModelOperationExceeded(limit: 2, actual: 3))
        }
    }

    func testMaxModelCountLimitEnforced() throws {
        let limits = StoreLimits().withMaxModelCount(2)
        var store = SemanticStore(limits: limits)
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())

        try store.createModel(id: ModelId(1), modelType: listType, itemCount: 100)
        try store.createModel(id: ModelId(2), modelType: listType, itemCount: 100)

        XCTAssertThrowsError(
            try store.createModel(id: ModelId(3), modelType: listType, itemCount: 100)
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxModelCountExceeded(limit: 2, current: 2))
        }
        XCTAssertEqual(store.modelCount, 2)
    }

    func testMaxCachedItemsPerModelEnforced() throws {
        let limits = StoreLimits().withMaxCachedItemsPerModel(3)
        var store = SemanticStore(limits: limits)
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)

        try store.createModel(id: modelID, modelType: listType, itemCount: 10_000)

        // Insert 2 items -> ok
        try store.modelInsert(
            id: modelID,
            index: 0,
            items: [
                ModelItem(itemID: ItemId(1), value: .string("a")),
                ModelItem(itemID: ItemId(2), value: .string("b"))
            ]
        )

        // Insert 2 more items -> projected cached is 4, which exceeds limit 3
        XCTAssertThrowsError(
            try store.modelInsert(
                id: modelID,
                index: 2,
                items: [
                    ModelItem(itemID: ItemId(3), value: .string("c")),
                    ModelItem(itemID: ItemId(4), value: .string("d"))
                ]
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxCachedItemsPerModelExceeded(limit: 3, current: 2, attempted: 4))
        }

        // Reset range with 4 items -> exceeds limit 3
        XCTAssertThrowsError(
            try store.modelResetRange(
                id: modelID,
                startIndex: 0,
                items: [
                    ModelItem(itemID: ItemId(10), value: .string("x0")),
                    ModelItem(itemID: ItemId(11), value: .string("x1")),
                    ModelItem(itemID: ItemId(12), value: .string("x2")),
                    ModelItem(itemID: ItemId(13), value: .string("x3"))
                ],
                totalCount: nil
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxCachedItemsPerModelExceeded(limit: 3, current: 2, attempted: 4))
        }
    }

    func testModelDeleteCombinedIdentityAndRangePreservesItemCount() throws {
        var store = SemanticStore()
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)

        try store.createModel(id: modelID, modelType: listType, itemCount: 100)
        try store.modelResetRange(
            id: modelID,
            startIndex: 5,
            items: [
                ModelItem(itemID: ItemId(50), value: .string("item-5")),
                ModelItem(itemID: ItemId(51), value: .string("item-6"))
            ],
            totalCount: nil
        )

        // Attempt invalid combined delete
        XCTAssertThrowsError(
            try store.modelDelete(
                id: modelID,
                index: 5,
                count: 2,
                itemIds: [ItemId(50)]
            )
        ) { error in
            guard case StoreError.invalidModelDelete = error else {
                XCTFail("Expected invalidModelDelete error")
                return
            }
        }

        // Verify item count and cache are completely untouched
        let model = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(model.itemCount, 100)
        XCTAssertEqual(model.cachedItemCount, 2)
        XCTAssertTrue(model.containsItem(ItemId(50)))
        XCTAssertTrue(model.containsItem(ItemId(51)))
    }

    func testModelDeleteSparseRangeLargeCount() throws {
        var store = SemanticStore()
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)
        let totalCount: UInt64 = 1_000_000

        try store.createModel(id: modelID, modelType: listType, itemCount: totalCount)

        // Place sparse items at index 10, 50, 100, 200
        try store.modelResetRange(
            id: modelID,
            startIndex: 10,
            items: [ModelItem(itemID: ItemId(10), value: .string("at-10"))],
            totalCount: nil
        )
        try store.modelResetRange(
            id: modelID,
            startIndex: 50,
            items: [ModelItem(itemID: ItemId(50), value: .string("at-50"))],
            totalCount: nil
        )
        try store.modelResetRange(
            id: modelID,
            startIndex: 100,
            items: [ModelItem(itemID: ItemId(100), value: .string("at-100"))],
            totalCount: nil
        )
        try store.modelResetRange(
            id: modelID,
            startIndex: 200,
            items: [ModelItem(itemID: ItemId(200), value: .string("at-200"))],
            totalCount: nil
        )

        // Delete range [40, 140) (count = 100). This covers items at 50 and 100.
        try store.modelDelete(id: modelID, index: 40, count: 100, itemIds: [])

        let model = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(model.itemCount, 999_900)
        XCTAssertEqual(model.cachedItemCount, 2)

        // Item 10 is untouched before range
        XCTAssertEqual(model.indexOf(ItemId(10)), 10)
        // Items 50 and 100 removed
        XCTAssertFalse(model.containsItem(ItemId(50)))
        XCTAssertFalse(model.containsItem(ItemId(100)))
        // Item 200 shifted down by 100 to index 100
        XCTAssertEqual(model.indexOf(ItemId(200)), 100)
    }

    func testModelBatchLimitsEnforcedAcrossAllOps() throws {
        let limits = StoreLimits().withMaxItemsPerModelOperation(2)
        var store = SemanticStore(limits: limits)
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())
        let modelID = ModelId(1)

        try store.createModel(id: modelID, modelType: listType, itemCount: 100)

        // 1. modelUpdate batch limit
        XCTAssertThrowsError(
            try store.modelUpdate(
                id: modelID,
                index: 0,
                items: [
                    ModelItem(itemID: ItemId(1), value: .string("a")),
                    ModelItem(itemID: ItemId(2), value: .string("b")),
                    ModelItem(itemID: ItemId(3), value: .string("c"))
                ]
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxItemsPerModelOperationExceeded(limit: 2, actual: 3))
        }

        // 2. modelResetRange batch limit
        XCTAssertThrowsError(
            try store.modelResetRange(
                id: modelID,
                startIndex: 0,
                items: [
                    ModelItem(itemID: ItemId(1), value: .string("a")),
                    ModelItem(itemID: ItemId(2), value: .string("b")),
                    ModelItem(itemID: ItemId(3), value: .string("c"))
                ],
                totalCount: nil
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxItemsPerModelOperationExceeded(limit: 2, actual: 3))
        }

        // 3. modelDelete batch limit (itemIds)
        XCTAssertThrowsError(
            try store.modelDelete(
                id: modelID,
                index: nil,
                count: nil,
                itemIds: [ItemId(1), ItemId(2), ItemId(3)]
            )
        ) { error in
            XCTAssertEqual(error as? StoreError, StoreError.maxItemsPerModelOperationExceeded(limit: 2, actual: 3))
        }
    }

    func testModelUpdateByIdentityAndByIndex() throws {
        var store = SemanticStore()
        let modelID = ModelId(1)
        let listType = try XCTUnwrap(resolveStandardNodeType("List").get())

        try store.createModel(id: modelID, modelType: listType, itemCount: 10)

        try store.modelInsert(
            id: modelID,
            index: 0,
            items: [
                ModelItem(itemID: ItemId(10), value: .string("original_10")),
                ModelItem(itemID: ItemId(20), value: .string("original_20"))
            ]
        )

        // 1. Update by identity (index: nil)
        try store.modelUpdate(
            id: modelID,
            index: nil,
            items: [ModelItem(itemID: ItemId(20), value: .string("updated_20_by_id"))]
        )

        let model1 = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(model1.getItemByIndex(1)?.value, .string("updated_20_by_id"))
        XCTAssertEqual(model1.getItemById(ItemId(20))?.value, .string("updated_20_by_id"))

        // 2. Update by index (index: 0)
        try store.modelUpdate(
            id: modelID,
            index: 0,
            items: [ModelItem(itemID: ItemId(10), value: .string("updated_10_by_idx"))]
        )

        let model2 = try XCTUnwrap(store.getModel(modelID))
        XCTAssertEqual(model2.getItemByIndex(0)?.value, .string("updated_10_by_idx"))
        XCTAssertEqual(model2.getItemById(ItemId(10))?.value, .string("updated_10_by_idx"))
    }
}
