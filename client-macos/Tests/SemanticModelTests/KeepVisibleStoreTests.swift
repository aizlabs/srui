//
// KeepVisibleStoreTests.swift
// SemanticModelTests
//
// Replica rules for the server-hinted keep-in-view item, the standard `keep_visible` property
// (§4 inv. 13, §6.3, §8, §12.1). They mirror the Rust store (`keep_visible_test.rs`) so both
// implementations reach the same verdict on the suite 1 keep_visible vectors.
//

import XCTest
@testable import SemanticModel

final class KeepVisibleStoreTests: XCTestCase {
    private let table = NodeId(2)
    private let model = ModelId(7)

    private func makeStore(_ nodeType: TypeRef = .table, modelBacked: Bool = true) throws -> SemanticStore {
        var store = SemanticStore()
        try store.createModel(id: model, modelType: .table, itemCount: 0)
        try store.modelInsert(
            id: model,
            index: 0,
            items: (1...3).map { ModelItem(itemID: ItemId(UInt64($0)), value: .string("row \($0)")) }
        )
        let props: [(PropertyRef, Value)] = modelBacked ? [(.modelRef, .unsignedInt(model.value))] : []
        try store.createNode(id: table, nodeType: nodeType, properties: props)
        return store
    }

    private func assertInvalid(_ body: () throws -> Void, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try body(), file: file, line: line) { error in
            guard case StoreError.invalidKeepVisible = error else {
                return XCTFail("expected invalidKeepVisible, got \(error)", file: file, line: line)
            }
            XCTAssertEqual((error as? StoreError)?.conformanceCode, "invalid_keep_visible", file: file, line: line)
        }
    }

    func testItemIdIsAdmittedOnEveryModelBackedCollectionType() throws {
        for nodeType in [TypeRef.list, .table, .tree] {
            var store = try makeStore(nodeType)
            try store.setProperty(nodeID: table, property: .keepVisible, value: .itemID(ItemId(2)))
            XCTAssertEqual(store.getNode(table)?.keepVisible, ItemId(2))
            // Never selection state.
            XCTAssertNil(store.getNode(table)?.promotedSelection)
            XCTAssertNil(store.getNode(table)?.getProperty(.selectionMode))
        }
    }

    func testUncachedItemIsNotAReplicaError() throws {
        var store = try makeStore()
        try store.setProperty(nodeID: table, property: .keepVisible, value: .itemID(ItemId(999)))
        XCTAssertEqual(store.getNode(table)?.keepVisible, ItemId(999))
    }

    func testRowIndexOrOtherValueIsRefused() throws {
        for value in [Value.unsignedInt(2), .signedInt(2), .string("2"), .bool(true), .null, .list([.itemID(ItemId(2))])] {
            var store = try makeStore()
            assertInvalid { try store.setProperty(nodeID: table, property: .keepVisible, value: value) }
            XCTAssertNil(store.getNode(table)?.getProperty(.keepVisible))
        }
    }

    func testNonCollectionOrModelLessNodesRefuseIt() throws {
        for nodeType in [TypeRef.button, .text, .select, .surface] {
            var store = try makeStore(nodeType, modelBacked: false)
            assertInvalid { try store.setProperty(nodeID: table, property: .keepVisible, value: .itemID(ItemId(1))) }
        }
        for nodeType in [TypeRef.list, .table, .tree] {
            var store = try makeStore(nodeType, modelBacked: false)
            assertInvalid { try store.setProperty(nodeID: table, property: .keepVisible, value: .itemID(ItemId(1))) }
            XCTAssertNil(store.getNode(table)?.keepVisible)
        }
    }

    func testClearingModelRefWhileHeldIsRefused() throws {
        var store = try makeStore()
        try store.setProperty(nodeID: table, property: .keepVisible, value: .itemID(ItemId(1)))
        assertInvalid { try store.clearProperty(nodeID: table, property: .modelRef) }
        XCTAssertNotNil(store.getNode(table)?.getProperty(.modelRef))
        try store.clearProperty(nodeID: table, property: .keepVisible)
        try store.clearProperty(nodeID: table, property: .modelRef)
    }

    func testCreateNodeAndBatchUseTheResultingPropertySet() throws {
        var store = SemanticStore()
        try store.createModel(id: model, modelType: .table, itemCount: 0)
        assertInvalid {
            try store.createNode(id: table, nodeType: .table, properties: [(.keepVisible, .itemID(ItemId(1)))])
        }
        assertInvalid {
            try store.createNode(
                id: table,
                nodeType: .button,
                properties: [(.modelRef, .unsignedInt(model.value)), (.keepVisible, .itemID(ItemId(1)))]
            )
        }
        XCTAssertNil(store.getNode(table))
        try store.createNode(
            id: table,
            nodeType: .table,
            properties: [(.keepVisible, .itemID(ItemId(1))), (.modelRef, .unsignedInt(model.value))]
        )
        XCTAssertEqual(store.getNode(table)?.keepVisible, ItemId(1))

        assertInvalid { try store.batchPropertySet(nodeID: table, properties: [(.keepVisible, .unsignedInt(3))]) }
        try store.batchPropertySet(
            nodeID: table,
            properties: [(.keepVisible, .unsignedInt(3)), (.keepVisible, .itemID(ItemId(3)))]
        )
        XCTAssertEqual(store.getNode(table)?.keepVisible, ItemId(3))
    }

    func testARefusedHintDiscardsTheWholeTransaction() throws {
        var store = try makeStore()
        let revision = store.revision
        let result = store.applyTransactionRecord(Transaction(
            baseRevision: revision,
            operations: [
                .setProperty(id: table, property: .label, value: .string("Processes")),
                .setProperty(id: table, property: .keepVisible, value: .unsignedInt(1)),
            ]
        ))
        guard case .failure(let error) = result else { return XCTFail("row index accepted") }
        XCTAssertEqual(error.conformanceCode, "invalid_keep_visible")
        XCTAssertEqual(store.revision, revision)
        XCTAssertNil(store.getNode(table)?.getProperty(.label))
    }

    func testTheHintIsIndependentOfSelectionRules() throws {
        var store = try makeStore()
        try store.setProperty(nodeID: table, property: .selectionMode, value: .enumToken(.selectionModeNone))
        try store.setProperty(nodeID: table, property: .keepVisible, value: .itemID(ItemId(2)))
        XCTAssertThrowsError(try store.setProperty(nodeID: table, property: .value, value: .itemID(ItemId(2)))) {
            guard case StoreError.invalidCollectionSelection = $0 else { return XCTFail("\($0)") }
        }
        XCTAssertEqual(store.getNode(table)?.keepVisible, ItemId(2))
    }
}
