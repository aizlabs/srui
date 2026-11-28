//
// SplitStoreTests.swift
// SemanticModelTests
//
// Replica and decode rules for the standard Split container (PX-070-G02; §4 inv. 13, §7.3, §10,
// §12.1, §26). They mirror the Rust store (`server-rust/semantic-tree/tests/split_test.rs`) so
// both implementations reach the same verdict on suite 1 vectors 95–103 and on the malformed
// vectors `malformed_split_*.bin` (suite 9).
//

import Foundation
import XCTest
@testable import Protocol
@testable import SemanticModel

final class SplitStoreTests: XCTestCase {
    private let surface = NodeId(1)
    private let split = NodeId(2)

    private func splitStore(_ properties: [(PropertyRef, Value)] = []) throws -> SemanticStore {
        var store = SemanticStore()
        try store.createNode(id: surface, nodeType: .surface)
        try store.createNode(id: split, nodeType: .split, parentID: surface, properties: properties)
        return store
    }

    private func addPanes(_ store: inout SemanticStore, parent: NodeId, from first: UInt64, count: Int) throws {
        for id in first..<(first + UInt64(count)) {
            try store.createNode(
                id: NodeId(id), nodeType: .text, parentID: parent,
                properties: [(.text, .string("pane \(id)"))]
            )
        }
    }

    private func code(_ body: () throws -> Void, file: StaticString = #filePath, line: UInt = #line) -> String? {
        do {
            try body()
            XCTFail("operation must be refused", file: file, line: line)
            return nil
        } catch let error as StoreError {
            return error.conformanceCode
        } catch {
            XCTFail("unexpected \(error)", file: file, line: line)
            return nil
        }
    }

    func testASplitAcceptsOneThroughEightPanesOnEitherAxis() throws {
        for axis: EnumToken? in [nil, .splitAxisSideBySide, .splitAxisStacked] {
            for panes in 1...maxSplitPanes {
                var store = try splitStore(axis.map { [(.splitAxis, .enumToken($0))] } ?? [])
                try addPanes(&store, parent: split, from: 10, count: panes)
                XCTAssertEqual(store.children(of: split)?.count, panes)
            }
        }
        XCTAssertEqual(maxSplitPanes, 8)
    }

    func testTheAxisDefaultsToSideBySideAndParsesOnlySplitAxisValues() {
        XCTAssertEqual(effectiveSplitAxis(nil), .sideBySide)
        XCTAssertEqual(effectiveSplitAxis(.enumToken(.splitAxisStacked)), .stacked)
        XCTAssertNil(effectiveSplitAxis(.enumToken(EnumToken(enumID: 13, valueID: 3))))
        guard case .failure(let unknown) = parseSplitAxis(.enumToken(EnumToken(enumID: 13, valueID: 3))) else {
            return XCTFail("13:3 accepted")
        }
        XCTAssertEqual(unknown, .unknownValue(enumID: 13, valueID: 3))
        XCTAssertEqual(
            unknown.description,
            "split_axis must be SplitAxis side_by_side (13:1) or stacked (13:2), got enum 13 value 3"
        )
        guard case .failure(let notEnum) = parseSplitAxis(.string("stacked")) else {
            return XCTFail("a string accepted")
        }
        XCTAssertEqual(
            notEnum.description,
            "split_axis must be SplitAxis side_by_side (13:1) or stacked (13:2), got a non-enum value"
        )
    }

    func testANinthPaneIsRefusedOnCreateAndRollsBackTheTransaction() throws {
        var store = try splitStore()
        try addPanes(&store, parent: split, from: 10, count: 8)
        let revision = store.revision
        let result = store.applyTransactionRecord(Transaction(
            baseRevision: revision,
            operations: [
                .setProperty(id: split, property: .label, value: .string("Dashboard")),
                .createNode(id: 30, nodeType: .text, parentID: surface),
                .createNode(id: 31, nodeType: .text, parentID: split),
            ]
        ))
        guard case .failure(let error) = result else { return XCTFail("ninth pane accepted") }
        guard case .opFailed(let index, let source) = error else { return XCTFail("\(error)") }
        XCTAssertEqual(index, 2)
        XCTAssertEqual(source, .maxSplitPanesExceeded(splitID: split, limit: maxSplitPanes))
        XCTAssertEqual(error.conformanceCode, "max_split_panes_exceeded")
        XCTAssertEqual(store.revision, revision)
        XCTAssertFalse(store.containsNode(30))
        XCTAssertFalse(store.isIDUsed(31))
        XCTAssertNil(store.getNode(split)?.getProperty(.label))
        XCTAssertEqual(store.children(of: split)?.count, 8)
        // An explicit child index does not get round the limit either.
        XCTAssertEqual(code { try store.createNode(id: 32, nodeType: .text, parentID: split, childIndex: 0) },
                       "max_split_panes_exceeded")
    }

    func testTheLimitAppliesToEachSplitAndFreesUpAfterADelete() throws {
        var store = try splitStore()
        try addPanes(&store, parent: split, from: 10, count: 8)
        try store.deleteNode(17)
        try store.createNode(id: 40, nodeType: .split, parentID: split)
        try addPanes(&store, parent: 40, from: 41, count: 8)
        XCTAssertEqual(code { try store.createNode(id: 49, nodeType: .text, parentID: 40) },
                       "max_split_panes_exceeded")
        try addPanes(&store, parent: surface, from: 60, count: 12)
    }

    func testMovingANinthPaneInIsRefusedButMovingWithinTheSplitIsNot() throws {
        var store = try splitStore()
        try addPanes(&store, parent: split, from: 10, count: 8)
        try store.createNode(id: 30, nodeType: .text, parentID: surface)
        XCTAssertEqual(code { try store.moveNode(nodeID: 30, newParentID: split, newChildIndex: nil) },
                       "max_split_panes_exceeded")
        XCTAssertEqual(store.parent(of: 30), .some(surface))
        try store.moveNode(nodeID: 17, newParentID: split, newChildIndex: 0)
        XCTAssertEqual(store.children(of: split)?.first, 17)
        try store.reorderChildren(parentID: split, newOrder: (10...17).reversed().map { NodeId(UInt64($0)) })
        try store.moveNode(nodeID: 10, newParentID: surface, newChildIndex: nil)
        try store.moveNode(nodeID: 30, newParentID: split, newChildIndex: nil)
    }

    func testSplitAxisIsAdmittedOnSplitAndRefusedOnEveryOtherNodeType() throws {
        let stacked: (PropertyRef, Value) = (.splitAxis, .enumToken(.splitAxisStacked))
        let node = NodeId(5)
        for entry in standardNodeTypesTable {
            let nodeType = TypeRef.standard(entry.id)
            var store = try splitStore()
            var setStore = try splitStore()
            try setStore.createNode(id: node, nodeType: nodeType, parentID: surface)
            if nodeType == .split {
                try store.createNode(id: node, nodeType: nodeType, parentID: surface, properties: [stacked])
                try setStore.setProperty(nodeID: node, property: .splitAxis, value: stacked.1)
                try setStore.batchPropertySet(nodeID: node, properties: [stacked])
                XCTAssertEqual(setStore.getNode(node)?.getProperty(.splitAxis), stacked.1)
                continue
            }
            let expected = StoreError.splitAxisNotAdmitted(nodeID: node, nodeType: nodeType)
            for (path, body) in [
                ("create", { try store.createNode(id: node, nodeType: nodeType, parentID: self.surface, properties: [stacked]) }),
                ("set", { _ = try setStore.setProperty(nodeID: node, property: .splitAxis, value: stacked.1) }),
                ("batch", { try setStore.batchPropertySet(nodeID: node, properties: [stacked]) }),
            ] as [(String, () throws -> Void)] {
                XCTAssertThrowsError(try body(), "\(entry.name) \(path)") { error in
                    XCTAssertEqual(error as? StoreError, expected, "\(entry.name) \(path)")
                    XCTAssertEqual((error as? StoreError)?.conformanceCode, "split_axis_not_admitted")
                }
            }
            XCTAssertFalse(store.containsNode(node), "\(entry.name): no partial create")
            XCTAssertNil(setStore.getNode(node)?.getProperty(.splitAxis))
        }
    }

    func testAValueOutsideSplitAxisIsRefusedAtApply() throws {
        let bad: [Value] = [
            .enumToken(EnumToken(enumID: 13, valueID: 0)),
            .enumToken(EnumToken(enumID: 13, valueID: 3)),
            .enumToken(.visibilityHidden),
            .string("stacked"),
            .unsignedInt(2),
            .null,
        ]
        for value in bad {
            var store = try splitStore()
            XCTAssertEqual(code { try store.setProperty(nodeID: split, property: .splitAxis, value: value) },
                           "invalid_split_axis", "\(value)")
            XCTAssertEqual(code { try store.batchPropertySet(nodeID: split, properties: [(.splitAxis, value)]) },
                           "invalid_split_axis")
            XCTAssertEqual(code { try store.createNode(id: 9, nodeType: .split, parentID: surface, properties: [(.splitAxis, value)]) },
                           "invalid_split_axis")
            // The value is checked before admission, as for shortcut: a Button gets the same code.
            try store.createNode(id: 10, nodeType: .button, parentID: surface)
            XCTAssertEqual(code { try store.setProperty(nodeID: 10, property: .splitAxis, value: value) },
                           "invalid_split_axis")
        }
        var store = try splitStore([(.splitAxis, .enumToken(.splitAxisStacked))])
        try store.clearProperty(nodeID: split, property: .splitAxis)
        XCTAssertEqual(effectiveSplitAxis(store.getNode(split)?.getProperty(.splitAxis)), .sideBySide)
    }

    func testNestedSplitsCountTowardTheTreeDepthLimit() throws {
        var store = SemanticStore(limits: StoreLimits().withMaxTreeDepth(4))
        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 2, nodeType: .split, parentID: 1)
        try store.createNode(id: 3, nodeType: .split, parentID: 2)
        try store.createNode(id: 4, nodeType: .text, parentID: 3)
        XCTAssertEqual(code { try store.createNode(id: 5, nodeType: .split, parentID: 4) },
                       "max_tree_depth_exceeded")
    }

    // MARK: - Decode paths (suite 9)

    func testEveryDecodePathRefusesAValueOutsideSplitAxis() throws {
        for value in [Value.enumToken(EnumToken(enumID: 13, valueID: 3)), .enumToken(EnumToken(enumID: 6, valueID: 1)), .string("stacked")] {
            let property = Property(property: .splitAxis, value: value).toWire()
            XCTAssertThrowsError(try Property(wire: property)) {
                XCTAssertEqual(($0 as? ValueConversionError).map { "\($0)".hasPrefix("invalid split_axis: ") }, true)
            }
            let set = StoreOperation.setProperty(id: split, property: .splitAxis, value: value).toWire()
            XCTAssertThrowsError(try StoreOperation(wire: set))
            XCTAssertThrowsError(try ProtocolDecoder().validateAndConvertOperation(wire: set)) {
                XCTAssertEqual(($0 as? ProtocolDecodeError)?.conformanceCode, "invalid_split_axis")
            }
            let batch = StoreOperation.batchPropertySet(
                id: split, properties: [Property(property: .splitAxis, value: value)]
            ).toWire()
            XCTAssertThrowsError(try StoreOperation(wire: batch))
            XCTAssertThrowsError(try ProtocolDecoder().validateAndConvertOperation(wire: batch))
            let create = StoreOperation.createNode(
                id: split, nodeType: .split, parentID: surface,
                properties: [Property(property: .splitAxis, value: value)]
            ).toWire()
            XCTAssertThrowsError(try StoreOperation(wire: create))
            XCTAssertThrowsError(try ProtocolDecoder().validateAndConvertOperation(wire: create))
        }
        for good in [EnumToken.splitAxisSideBySide, .splitAxisStacked] {
            let op = StoreOperation.setProperty(id: split, property: .splitAxis, value: .enumToken(good))
            XCTAssertEqual(try StoreOperation(wire: op.toWire()), op)
            XCTAssertEqual(try ProtocolDecoder().validateAndConvertOperation(wire: op.toWire()), op)
        }
    }

    private func malformedVector(_ name: String) throws -> (wire: Srui_Protocol_Transaction, spec: [String: Any]) {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("protocol/conformance-vectors")
        let expected = try JSONSerialization.jsonObject(
            with: Data(contentsOf: root.appendingPathComponent("expected.json"))
        ) as? [String: Any]
        let malformed = try XCTUnwrap(expected?["malformed_vectors"] as? [String: Any])
        let spec = try XCTUnwrap(malformed[name] as? [String: Any], name)
        let bytes = try Data(contentsOf: root.appendingPathComponent("\(name).bin"))
        XCTAssertEqual(bytes.count, spec["byte_length"] as? Int, name)
        let message = try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: bytes)
        guard case .transaction(let wire)? = message.msg else {
            throw XCTSkip("unreachable: \(name) must hold a transaction")
        }
        return (wire, spec)
    }

    /// The unknown axis value is refused by both Swift decode paths with the Rust reason; the
    /// ninth pane decodes on both paths and is refused at apply with nothing committed.
    func testTheMalformedSplitVectorsAreRefused() throws {
        let unknown = try malformedVector("malformed_split_axis_unknown_value")
        let rustError = try XCTUnwrap(unknown.spec["expected_error"] as? String)
        XCTAssertThrowsError(try ProtocolDecoder().validateAndConvertTransaction(wire: unknown.wire)) { error in
            let decodeError = error as? ProtocolDecodeError
            XCTAssertEqual(decodeError?.conformanceCode, "invalid_split_axis")
            // Rust prefixes its transaction wrapper; the reason after it is identical.
            XCTAssertEqual("wire transaction error: \(decodeError?.description ?? "")", rustError)
        }
        XCTAssertThrowsError(try Transaction(wire: unknown.wire)) { error in
            XCTAssertEqual(
                "wire transaction error: \(error)", rustError,
                "the wire-conversion path reports the same reason"
            )
        }

        let ninth = try malformedVector("malformed_split_ninth_pane")
        XCTAssertEqual(ninth.spec["expected_error_code"] as? String, "max_split_panes_exceeded")
        let failedIndex = try XCTUnwrap(ninth.spec["failed_op_index"] as? Int)
        for decoded in [
            try ProtocolDecoder().validateAndConvertTransaction(wire: ninth.wire),
            try Transaction(wire: ninth.wire),
        ] {
            XCTAssertEqual(decoded.operations.count, 11)
            var store = SemanticStore()
            let result = store.applyTransactionRecord(decoded)
            guard case .failure(let error) = result, case .opFailed(let index, _) = error else {
                return XCTFail("ninth pane accepted: \(result)")
            }
            XCTAssertEqual(index, failedIndex)
            XCTAssertEqual(error.conformanceCode, "max_split_panes_exceeded")
            XCTAssertTrue(store.isEmpty)
            XCTAssertEqual(store.revision, .initial)
        }
    }
}
