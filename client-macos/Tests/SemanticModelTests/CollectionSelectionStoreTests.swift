//
// CollectionSelectionStoreTests.swift
// SemanticModelTests
//
// Replica rules for a server-promoted collection selection (§6.3, §7.4, §8, §12.1). They match
// the Rust store so both implementations reach the same verdict on conformance vectors 50–53.
//

import XCTest
@testable import SemanticModel

final class CollectionSelectionStoreTests: XCTestCase {
    private let table = NodeId(2)
    private let model = ModelId(7)

    private func makeStore(_ nodeType: TypeRef = .table, mode: Value? = .enumToken(.selectionModeSingle)) throws -> SemanticStore {
        var store = SemanticStore()
        try store.createModel(id: model, modelType: .table, itemCount: 0)
        try store.modelInsert(
            id: model,
            index: 0,
            items: (1...3).map { ModelItem(itemID: ItemId(UInt64($0)), value: .string("row \($0)")) }
        )
        var props: [(PropertyRef, Value)] = [(.modelRef, .unsignedInt(model.value))]
        if let mode { props.append((.selectionMode, mode)) }
        try store.createNode(id: table, nodeType: nodeType, properties: props)
        return store
    }

    private func assertInvalid(_ body: () throws -> Void, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try body(), file: file, line: line) { error in
            guard case StoreError.invalidCollectionSelection = error else {
                return XCTFail("expected invalidCollectionSelection, got \(error)", file: file, line: line)
            }
            XCTAssertEqual((error as? StoreError)?.conformanceCode, "invalid_collection_selection", file: file, line: line)
        }
    }

    func testItemIdValueIsThePromotedSelectionOfEveryCollectionType() throws {
        for nodeType in [TypeRef.list, .table, .tree] {
            var store = try makeStore(nodeType)
            try store.setProperty(nodeID: table, property: .value, value: .itemID(ItemId(2)))
            XCTAssertEqual(store.getNode(table)?.promotedSelection, ItemId(2))
        }
    }

    func testUncachedItemIsNotAReplicaError() throws {
        var store = try makeStore()
        try store.setProperty(nodeID: table, property: .value, value: .itemID(ItemId(999)))
        XCTAssertEqual(store.getNode(table)?.promotedSelection, ItemId(999))
    }

    func testRowIndexOrOtherValueIsRefused() throws {
        for value in [Value.unsignedInt(2), .signedInt(2), .string("2"), .list([.itemID(ItemId(2))])] {
            var store = try makeStore()
            assertInvalid { try store.setProperty(nodeID: table, property: .value, value: value) }
            XCTAssertNil(store.getNode(table)?.getProperty(.value))
        }
    }

    func testSelectionModeNoneOrAbsentRefusesPromotion() throws {
        for mode: Value? in [.enumToken(.selectionModeNone), nil] as [Value?] {
            var store = try makeStore(mode: mode)
            assertInvalid { try store.setProperty(nodeID: table, property: .value, value: .itemID(ItemId(1))) }
        }
    }

    func testDisablingSelectionWhilePromotedIsRefused() throws {
        var store = try makeStore()
        try store.setProperty(nodeID: table, property: .value, value: .itemID(ItemId(1)))
        assertInvalid { try store.setProperty(nodeID: table, property: .selectionMode, value: .enumToken(.selectionModeNone)) }
        assertInvalid { try store.clearProperty(nodeID: table, property: .selectionMode) }
        assertInvalid { try store.batchPropertySet(nodeID: table, properties: [(.selectionMode, .enumToken(.selectionModeNone))]) }
        try store.clearProperty(nodeID: table, property: .value)
        try store.setProperty(nodeID: table, property: .selectionMode, value: .enumToken(.selectionModeNone))
    }

    func testCreateNodeUsesTheResultingPropertySet() throws {
        var store = SemanticStore()
        try store.createModel(id: model, modelType: .table, itemCount: 0)
        assertInvalid {
            try store.createNode(id: table, nodeType: .table, properties: [(.value, .itemID(ItemId(1)))])
        }
        XCTAssertNil(store.getNode(table))
        try store.createNode(
            id: table,
            nodeType: .table,
            properties: [(.value, .itemID(ItemId(1))), (.selectionMode, .enumToken(.selectionModeMultiple))]
        )
        XCTAssertEqual(store.getNode(table)?.promotedSelection, ItemId(1))
    }

    func testValueOnNonCollectionNodesIsUnaffected() throws {
        var store = SemanticStore()
        try store.createNode(id: NodeId(1), nodeType: .textInput, properties: [(.value, .string("abc"))])
        try store.setProperty(nodeID: NodeId(1), property: .value, value: .itemID(ItemId(1)))
        XCTAssertNil(store.getNode(NodeId(1))?.promotedSelection)
    }
}
