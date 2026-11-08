//
// ShortcutStoreTests.swift
// SemanticModelTests
//
// Replica-side rules for the standard `shortcut` property (PX-016-G01), in parity with
// `server-rust/semantic-tree/src/shortcut.rs` and `tests/shortcut_test.rs`: the closed grammar,
// node-type admission, uniqueness within one Surface (including MOVE_NODE), and the decode check
// on the Protocol wire conversions. Implements: §4 inv. 13, §7.4, §7.7, §12.1, §22.5, §26.
// §32 suites 2, 6 and 9 run this file; suite 1 vectors 54–66 replay the same rules in both
// languages.
//

import XCTest
@testable import Protocol
@testable import SemanticModel

final class ShortcutStoreTests: XCTestCase {

    // MARK: - Grammar (identical case table to the Rust unit tests)

    func testCanonicalValuesRoundTrip() throws {
        for value in [
            "primary+r", "primary+shift+r", "primary+alt+r", "primary+shift+alt+r", "primary+0",
            "primary+9", "primary+return", "primary+shift+alt+page_down", "f1", "f12",
            "shift+f5", "alt+f3", "primary+f10",
        ] {
            XCTAssertEqual(try Shortcut(parsing: value).canonical, value)
        }
    }

    func testNonCanonicalValuesFailWithTheRustReason() {
        let cases: [(String, ShortcutError)] = [
            ("", .empty),
            ("primary+shift+alt+page_down_xxxxx", .tooLong(length: 33)),
            ("cmd+r", .unknownToken("cmd")),
            ("primary+R", .unknownToken("R")),
            ("Primary+r", .unknownToken("Primary")),
            ("primary+", .unknownToken("")),
            ("+r", .unknownToken("")),
            ("primary++r", .unknownToken("")),
            ("primary+f13", .unknownToken("f13")),
            ("primary+f01", .unknownToken("f01")),
            ("primary+comma", .unknownToken("comma")),
            ("primary+primary+r", .duplicateModifier("primary")),
            ("primary+shift+shift", .duplicateModifier("shift")),
            ("shift+primary+r", .misorderedModifier("primary")),
            ("primary+shift", .missingKey),
            ("r", .requiresPrimary("r")),
            ("shift+return", .requiresPrimary("return")),
            ("primary+é", .unknownToken("é")),
        ]
        for (value, expected) in cases {
            XCTAssertThrowsError(try Shortcut(parsing: value), value) { error in
                XCTAssertEqual(error as? ShortcutError, expected, value)
            }
        }
        XCTAssertThrowsError(try Shortcut(value: .bool(true))) { error in
            XCTAssertEqual(error as? ShortcutError, .notAString)
        }
        // Same wording as the Rust Display, so diagnostics read identically on both sides.
        XCTAssertEqual(
            ShortcutError.tooLong(length: 33).description,
            "a shortcut is at most 32 bytes, got 33"
        )
        XCTAssertEqual(ShortcutError.unknownToken("cmd").description, "unknown shortcut token \"cmd\"")
    }

    func testTheLengthBoundIsCheckedBeforeParsing() {
        XCTAssertThrowsError(try Shortcut(parsing: String(repeating: "x", count: 33))) {
            XCTAssertEqual($0 as? ShortcutError, .tooLong(length: 33))
        }
        let atBound = String(repeating: "y", count: maxShortcutBytes)
        XCTAssertThrowsError(try Shortcut(parsing: atBound)) {
            XCTAssertEqual($0 as? ShortcutError, .unknownToken(atBound))
        }
        // Multi-byte characters count bytes, not characters.
        XCTAssertThrowsError(try Shortcut(parsing: String(repeating: "é", count: 17))) {
            XCTAssertEqual($0 as? ShortcutError, .tooLong(length: 34))
        }
    }

    func testDisplayStringsFollowPlatformConventions() throws {
        let shortcut = try Shortcut(parsing: "primary+shift+alt+r")
        XCTAssertEqual(shortcut.displayString(for: .macOS), "⌥⇧⌘R")
        XCTAssertEqual(shortcut.displayString(for: .other), "Ctrl+Shift+Alt+R")
        XCTAssertEqual(try Shortcut(parsing: "f5").displayString(for: .macOS), "F5")
        XCTAssertEqual(try Shortcut(parsing: "primary+page_up").displayString(for: .other), "Ctrl+Page Up")
    }

    // MARK: - Store admission, uniqueness and atomicity (suite 2)

    private func surfaceStore() throws -> SemanticStore {
        var store = SemanticStore()
        try store.createNode(id: 1, nodeType: .surface)
        return store
    }

    private func code(_ body: () throws -> Void) -> String? {
        do {
            try body()
            return nil
        } catch let error as StoreError {
            return error.conformanceCode
        } catch {
            return "unexpected \(error)"
        }
    }

    func testEveryAdmittedTypeAcceptsAShortcutAndEveryOtherTypeRefusesIt() throws {
        let admitted: [(TypeRef, ShortcutBehavior)] = [
            (.button, .activate), (.toggle, .toggle), (.textInput, .focus), (.textArea, .focus),
            (.list, .focus), (.table, .focus), (.tree, .focus), (.richText, .focus),
        ]
        var store = try surfaceStore()
        for (index, entry) in admitted.enumerated() {
            XCTAssertEqual(ShortcutBehavior(nodeType: entry.0), entry.1)
            try store.createNode(
                id: NodeId(UInt64(10 + index)),
                nodeType: entry.0,
                parentID: 1,
                properties: [(.shortcut, .string("primary+\(index)"))]
            )
        }
        for (id, name, _) in standardNodeTypesTable {
            let nodeType = TypeRef.standard(id)
            if admitted.contains(where: { $0.0 == nodeType }) { continue }
            XCTAssertNil(ShortcutBehavior(nodeType: nodeType), name)
            var store = try surfaceStore()
            XCTAssertEqual(code {
                try store.createNode(
                    id: 100, nodeType: nodeType, parentID: 1,
                    properties: [(.shortcut, .string("primary+r"))]
                )
            }, "shortcut_not_admitted", name)
            try store.createNode(id: 100, nodeType: nodeType, parentID: 1)
            XCTAssertEqual(code {
                try store.setProperty(nodeID: 100, property: .shortcut, value: .string("primary+r"))
            }, "shortcut_not_admitted", name)
            XCTAssertEqual(code {
                try store.batchPropertySet(nodeID: 100, properties: [(.shortcut, .string("primary+r"))])
            }, "shortcut_not_admitted", name)
        }
    }

    func testMalformedValuesAreRefusedOnEverySettingPath() throws {
        var store = try surfaceStore()
        try store.createNode(id: 2, nodeType: .button, parentID: 1)
        for bad: Value in [
            .string("r"), .string("cmd+r"), .string("primary+primary+r"),
            .string("shift+primary+r"), .string("primary+shift+alt+page_down+xxxxx"),
            .string(""), .bool(true), .null,
        ] {
            XCTAssertEqual(code { try store.setProperty(nodeID: 2, property: .shortcut, value: bad) },
                           "invalid_shortcut", "\(bad)")
            XCTAssertEqual(code { try store.batchPropertySet(nodeID: 2, properties: [(.shortcut, bad)]) },
                           "invalid_shortcut")
            XCTAssertEqual(code {
                try store.createNode(id: 3, nodeType: .button, parentID: 1, properties: [(.shortcut, bad)])
            }, "invalid_shortcut")
        }
        XCTAssertNil(store.getNode(2)?.getProperty(.shortcut))
        XCTAssertFalse(store.containsNode(3))
    }

    func testABareTypingKeyNeverBindsButABareFunctionKeyDoes() throws {
        var store = try surfaceStore()
        try store.createNode(id: 2, nodeType: .textInput, parentID: 1)
        for bare in ["a", "z", "0", "space", "tab", "return", "delete", "up", "shift+a", "alt+a"] {
            XCTAssertEqual(code {
                try store.setProperty(nodeID: 2, property: .shortcut, value: .string(bare))
            }, "invalid_shortcut", bare)
        }
        XCTAssertNoThrow(try store.setProperty(nodeID: 2, property: .shortcut, value: .string("f6")))
    }

    func testShortcutsAreUniqueWithinOneSurfaceIncludingMoves() throws {
        var store = try surfaceStore()
        try store.createNode(id: 2, nodeType: .button, parentID: 1, properties: [(.shortcut, .string("primary+r"))])
        try store.createNode(id: 4, nodeType: .column, parentID: 1)
        XCTAssertEqual(code {
            try store.createNode(id: 3, nodeType: .toggle, parentID: 4, properties: [(.shortcut, .string("primary+r"))])
        }, "duplicate_shortcut")
        try store.createNode(id: 3, nodeType: .toggle, parentID: 4, properties: [(.shortcut, .string("primary+t"))])
        XCTAssertThrowsError(try store.setProperty(nodeID: 3, property: .shortcut, value: .string("primary+r"))) {
            XCTAssertEqual($0 as? StoreError, .duplicateShortcut(nodeID: 3, existing: 2, shortcut: "primary+r"))
        }
        XCTAssertNoThrow(try store.setProperty(nodeID: 2, property: .shortcut, value: .string("primary+r")))

        // Another Surface, and a nested Surface, are separate scopes.
        try store.createNode(id: 10, nodeType: .surface)
        try store.createNode(id: 11, nodeType: .column, parentID: 10)
        try store.createNode(id: 12, nodeType: .button, parentID: 11, properties: [(.shortcut, .string("primary+r"))])
        try store.createNode(id: 20, nodeType: .surface, parentID: 1)
        try store.createNode(id: 21, nodeType: .button, parentID: 20, properties: [(.shortcut, .string("primary+r"))])

        XCTAssertThrowsError(try store.moveNode(nodeID: 11, newParentID: 1, newChildIndex: nil)) {
            XCTAssertEqual($0 as? StoreError, .duplicateShortcut(nodeID: 12, existing: 2, shortcut: "primary+r"))
        }
        XCTAssertEqual(store.getNode(11)?.parentID, 10)
        try store.createNode(id: 13, nodeType: .column, parentID: 10)
        XCTAssertNoThrow(try store.moveNode(nodeID: 11, newParentID: 13, newChildIndex: nil))
        XCTAssertNoThrow(try store.moveNode(nodeID: 11, newParentID: nil, newChildIndex: nil))
        XCTAssertNoThrow(try store.moveNode(nodeID: 20, newParentID: 10, newChildIndex: nil))
        try store.clearProperty(nodeID: 2, property: .shortcut)
        XCTAssertNoThrow(try store.moveNode(nodeID: 11, newParentID: 1, newChildIndex: nil))
    }

    func testARefusedShortcutDiscardsTheWholeTransaction() throws {
        var store = try surfaceStore()
        try store.createNode(id: 2, nodeType: .button, parentID: 1)
        let revision = store.revision
        let result = store.applyTransactionRecord(Transaction(
            baseRevision: revision,
            operations: [
                .setProperty(id: 2, property: .label, value: .string("Refresh")),
                .setProperty(id: 2, property: .shortcut, value: .string("r")),
            ]
        ))
        guard case .failure(let error) = result else { return XCTFail("bare key accepted") }
        XCTAssertEqual(error.conformanceCode, "invalid_shortcut")
        XCTAssertEqual(store.revision, revision)
        XCTAssertNil(store.getNode(2)?.getProperty(.label))
    }

    // MARK: - Wire conversions (suite 9; ProtocolDecoder is covered in DecoderTests)

    func testWireConversionsRefuseAMalformedShortcut() throws {
        for bad in ["cmd+r", "primary+primary+r", "primary+shift+alt+page_down+xxxxx", "r"] {
            let property = Property(property: .shortcut, value: .string(bad)).toWire()
            XCTAssertThrowsError(try Property(wire: property), bad) {
                XCTAssertEqual(($0 as? ValueConversionError).map { "\($0)".hasPrefix("invalid shortcut: ") }, true)
            }
            let set = StoreOperation.setProperty(id: 2, property: .shortcut, value: .string(bad)).toWire()
            XCTAssertThrowsError(try StoreOperation(wire: set), bad)
            XCTAssertThrowsError(try ProtocolDecoder().validateAndConvertOperation(wire: set), bad) {
                XCTAssertEqual(($0 as? ProtocolDecodeError)?.conformanceCode, "invalid_shortcut")
            }
            let batch = StoreOperation.batchPropertySet(
                id: 2, properties: [Property(property: .shortcut, value: .string(bad))]
            ).toWire()
            XCTAssertThrowsError(try StoreOperation(wire: batch), bad)
            XCTAssertThrowsError(try ProtocolDecoder().validateAndConvertOperation(wire: batch), bad)
        }
        let canonical = StoreOperation.setProperty(id: 2, property: .shortcut, value: .string("primary+shift+r"))
        XCTAssertEqual(try StoreOperation(wire: canonical.toWire()), canonical)
        XCTAssertEqual(try ProtocolDecoder().validateAndConvertOperation(wire: canonical.toWire()), canonical)
    }
}
