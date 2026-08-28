import XCTest
import Foundation
@testable import Protocol
@testable import SemanticModel

final class SemanticModelTests: XCTestCase {

    // MARK: - Value Tests (Mirroring Task 3)

    func testConstructAndCompareAll17ValueVariants() throws {
        let sampleHash = try ResourceHash(rawBytes: Array(repeating: 0xab, count: 32))

        let vals: [Value] = [
            .null,
            .bool(true),
            .signedInt(-42),
            .unsignedInt(42),
            .float64(3.14159),
            .string("hello world"),
            .nodeID(NodeId(10)),
            .itemID(ItemId(20)),
            .resourceHash(sampleHash),
            .enumToken(EnumToken(enumID: 1, valueID: 2)),
            .size(Size(width: 100.0, height: 50.0)),
            .point(Point(x: 10.0, y: 20.0)),
            .range(SemanticRange(start: 0, length: 100)),
            .rect(Rect(x: 0.0, y: 0.0, width: 100.0, height: 50.0)),
            .edgeInsets(EdgeInsets(top: 8.0, leading: 12.0, bottom: 8.0, trailing: 12.0)),
            .list([.bool(true), .bool(false)]),
            .record(SmallRecord(
                typeRef: TypeRef(namespaceID: 0, localID: 1),
                properties: [
                    Property(property: PropertyRef(namespaceID: 0, localID: 1), value: .string("foo"))
                ]
            ))
        ]

        XCTAssertEqual(vals.len_count, 17, "Must contain exactly 17 distinct Value variants")

        for (i, v1) in vals.enumerated() {
            for (j, v2) in vals.enumerated() {
                if i == j {
                    XCTAssertEqual(v1, v2, "Value variant \(i) must equal itself")
                } else {
                    XCTAssertNotEqual(v1, v2, "Value variant \(i) must not equal variant \(j)")
                }
            }
        }
    }

    func testProtobufWireRoundtripAllVariants() throws {
        let sampleHash = try ResourceHash(rawBytes: Array(repeating: 0xab, count: 32))

        let vals: [Value] = [
            .null,
            .bool(true),
            .signedInt(-42),
            .unsignedInt(42),
            .float64(3.14159),
            .string("hello world"),
            .nodeID(NodeId(10)),
            .itemID(ItemId(20)),
            .resourceHash(sampleHash),
            .enumToken(EnumToken(enumID: 1, valueID: 2)),
            .size(Size(width: 100.0, height: 50.0)),
            .point(Point(x: 10.0, y: 20.0)),
            .range(SemanticRange(start: 0, length: 100)),
            .rect(Rect(x: 0.0, y: 0.0, width: 100.0, height: 50.0)),
            .edgeInsets(EdgeInsets(top: 8.0, leading: 12.0, bottom: 8.0, trailing: 12.0)),
            .list([.bool(true), .signedInt(123)]),
            .record(SmallRecord(
                typeRef: TypeRef(namespaceID: 0, localID: 1),
                properties: [
                    Property(property: PropertyRef(namespaceID: 0, localID: 1), value: .string("foo"))
                ]
            ))
        ]

        for val in vals {
            let wire = val.toWire()
            let back = try Value(wire: wire)
            XCTAssertEqual(val, back, "Protobuf roundtrip failed for variant: \(val)")
        }
    }

    func testValueHelperPropertiesAndLiterals() {
        let vNull: Value = nil
        XCTAssertTrue(vNull.isNull)
        XCTAssertTrue(vNull.isScalar)

        let vBool: Value = true
        XCTAssertEqual(vBool.asBool, true)
        XCTAssertTrue(vBool.isScalar)

        let vInt: Value = -100
        XCTAssertEqual(vInt.asSignedInt, -100)

        let vFloat: Value = 42.5
        XCTAssertEqual(vFloat.asFloat64, 42.5)

        let vString: Value = "SRUI"
        XCTAssertEqual(vString.asString, "SRUI")

        let vList: Value = [.bool(true), .bool(false)]
        XCTAssertFalse(vList.isScalar)
        XCTAssertEqual(vList.asList?.count, 2)
    }

    // MARK: - Identifier Tests (Mirroring Task 3)

    func testNodeIdNewtype() {
        let id1 = NodeId(42)
        let id2 = NodeId(42)
        let id3 = NodeId(43)

        XCTAssertEqual(id1, id2)
        XCTAssertNotEqual(id1, id3)
        XCTAssertEqual(id1.value, 42)
        XCTAssertEqual(id1.description, "NodeId(42)")
        XCTAssertTrue(id1 < id3)

        let literalId: NodeId = 42
        XCTAssertEqual(id1, literalId)
    }

    func testItemIdNewtype() {
        let id1 = ItemId(100)
        let id2 = ItemId(100)
        let id3 = ItemId(101)

        XCTAssertEqual(id1, id2)
        XCTAssertNotEqual(id1, id3)
        XCTAssertEqual(id1.value, 100)
        XCTAssertEqual(id1.description, "ItemId(100)")
        XCTAssertTrue(id1 < id3)

        let literalId: ItemId = 100
        XCTAssertEqual(id1, literalId)
    }

    func testModelIdNewtype() {
        let id1 = ModelId(10)
        let id2 = ModelId(10)
        let id3 = ModelId(11)

        XCTAssertEqual(id1, id2)
        XCTAssertNotEqual(id1, id3)
        XCTAssertEqual(id1.value, 10)
        XCTAssertEqual(id1.description, "ModelId(10)")
        XCTAssertTrue(id1 < id3)
    }

    func testTypeRefEqualityAndHashing() {
        let t1 = TypeRef(namespaceID: 0, localID: 1)
        let t2 = TypeRef.standard(1)
        let t3 = TypeRef(namespaceID: 1, localID: 1)

        XCTAssertEqual(t1, t2)
        XCTAssertNotEqual(t1, t3)
        XCTAssertTrue(t1.isStandard)
        XCTAssertFalse(t3.isStandard)
        XCTAssertEqual(t1.standardName, "Surface")
        XCTAssertNil(t3.standardName)
    }

    func testPropertyRefEqualityAndHashing() {
        let p1 = PropertyRef(namespaceID: 0, localID: 1)
        let p2 = PropertyRef.standard(1)
        let p3 = PropertyRef(namespaceID: 1, localID: 1)

        XCTAssertEqual(p1, p2)
        XCTAssertNotEqual(p1, p3)
        XCTAssertTrue(p1.isStandard)
        XCTAssertFalse(p3.isStandard)
        XCTAssertEqual(p1.standardName, "label")
        XCTAssertNil(p3.standardName)
    }

    func testWireProtoConversion() throws {
        let typeRef = TypeRef(namespaceID: 0, localID: 11)
        let wireType = typeRef.toWire()
        XCTAssertEqual(wireType.namespaceID, 0)
        XCTAssertEqual(wireType.localID, 11)
        let backType = TypeRef(wire: wireType)
        XCTAssertEqual(typeRef, backType)

        let propRef = PropertyRef(namespaceID: 0, localID: 1)
        let wireProp = propRef.toWire()
        XCTAssertEqual(wireProp.namespaceID, 0)
        XCTAssertEqual(wireProp.localID, 1)
        let backProp = PropertyRef(wire: wireProp)
        XCTAssertEqual(propRef, backProp)

        // Valid Property roundtrip
        let property = Property(property: PropertyRef.standard(1), value: .string("Hello"))
        let wireProperty = property.toWire()
        let backProperty = try Property(wire: wireProperty)
        XCTAssertEqual(property, backProperty)

        // Missing property field must throw ValueConversionError.missingField("property")
        var emptyWireProperty = SRUIProperty()
        XCTAssertThrowsError(try Property(wire: emptyWireProperty)) { error in
            guard case ValueConversionError.missingField(let field) = error else {
                XCTFail("Expected missingField error, got \(error)")
                return
            }
            XCTAssertEqual(field, "property")
        }

        // Missing value field defaults to Value.null
        emptyWireProperty.property = PropertyRef.standard(1).toWire()
        let defaultedProperty = try Property(wire: emptyWireProperty)
        XCTAssertEqual(defaultedProperty.property, PropertyRef.standard(1))
        XCTAssertEqual(defaultedProperty.value, .null)
    }

    func testResolveKnownRegistryNodeTypes() {
        XCTAssertEqual(try resolveStandardNodeType("Surface").get(), TypeRef.surface)
        XCTAssertEqual(try resolveStandardNodeType("Surface").get(), TypeRef.SURFACE)
        XCTAssertEqual(try resolveStandardNodeType("Button").get(), TypeRef.button)
        XCTAssertEqual(try resolveStandardNodeType("Button").get(), TypeRef.BUTTON)
        XCTAssertEqual(try resolveStandardNodeType("Text").get(), TypeRef.text)
        XCTAssertEqual(try resolveStandardNodeType("Text").get(), TypeRef.TEXT)
        XCTAssertEqual(try resolveStandardNodeType("RichText").get(), TypeRef.richText)
        XCTAssertEqual(try resolveStandardNodeType("RichText").get(), TypeRef.RICHTEXT)
    }

    func testResolveKnownRegistryProperties() {
        XCTAssertEqual(try resolveStandardProperty("label").get(), PropertyRef.label)
        XCTAssertEqual(try resolveStandardProperty("label").get(), PropertyRef.LABEL)
        XCTAssertEqual(try resolveStandardProperty("enabled").get(), PropertyRef.enabled)
        XCTAssertEqual(try resolveStandardProperty("enabled").get(), PropertyRef.ENABLED)
        XCTAssertEqual(try resolveStandardProperty("text").get(), PropertyRef.text)
        XCTAssertEqual(try resolveStandardProperty("text").get(), PropertyRef.TEXT)
    }

    func testResolveUnknownNameFailsClearly() {
        switch resolveStandardNodeType("NonExistentWidget") {
        case .failure(let err):
            XCTAssertEqual(err, .unknownNodeType("NonExistentWidget"))
        case .success:
            XCTFail("Expected failure for unknown node type")
        }

        switch resolveStandardProperty("non_existent_prop") {
        case .failure(let err):
            XCTAssertEqual(err, .unknownProperty("non_existent_prop"))
        case .success:
            XCTFail("Expected failure for unknown property")
        }

        switch resolveStandardEvent("non_existent_event") {
        case .failure(let err):
            XCTAssertEqual(err, .unknownEvent("non_existent_event"))
        case .success:
            XCTFail("Expected failure for unknown event")
        }
    }

    func testResourceHash() throws {
        let bytes = [UInt8](repeating: 7, count: 32)
        let hash = try ResourceHash(rawBytes: bytes)
        XCTAssertEqual(hash.asBytes, bytes)
        let hex = hash.toHex()
        XCTAssertEqual(hex, "0707070707070707070707070707070707070707070707070707070707070707")

        let parsed = try ResourceHash(hex: hex)
        XCTAssertEqual(hash, parsed)

        let prefixed = "sha256:\(hex)"
        let parsedPrefixed = try ResourceHash(hex: prefixed)
        XCTAssertEqual(hash, parsedPrefixed)
        XCTAssertEqual(hash.description, prefixed)

        // Invalid length
        XCTAssertThrowsError(try ResourceHash(hex: "0707")) { error in
            guard case ParseResourceHashError.invalidLength(let len) = error else {
                XCTFail("Expected invalidLength error")
                return
            }
            XCTAssertEqual(len, 4)
        }

        // Invalid hex characters
        let invalidHex = String(repeating: "z", count: 64)
        XCTAssertThrowsError(try ResourceHash(hex: invalidHex)) { error in
            guard case ParseResourceHashError.invalidHexCharacter = error else {
                XCTFail("Expected invalidHexCharacter error")
                return
            }
        }
    }

    func testResolveStandardEnumValues() {
        XCTAssertEqual(
            resolveStandardEnumValue(enumName: "ActionRole", valueName: "destructive"),
            EnumToken.actionRoleDestructive
        )
        XCTAssertEqual(
            resolveStandardEnumValue(enumName: "ActionRole", valueName: "destructive"),
            EnumToken.ACTION_ROLE_DESTRUCTIVE
        )
        XCTAssertEqual(
            resolveStandardEnumValue(enumName: "EnumActionRole", valueName: "primary"),
            EnumToken.actionRolePrimary
        )
        XCTAssertEqual(
            resolveStandardEnumValue(enumName: "EnumActionRole", valueName: "primary"),
            EnumToken.ACTION_ROLE_PRIMARY
        )
        XCTAssertEqual(
            resolveStandardEnumValue(enumName: "Visibility", valueName: "collapsed"),
            EnumToken.visibilityCollapsed
        )
        XCTAssertEqual(
            resolveStandardEnumValue(enumName: "Visibility", valueName: "collapsed"),
            EnumToken.VISIBILITY_COLLAPSED
        )
        XCTAssertEqual(
            lookupStandardEnumValue(enumID: 2, valueName: "destructive"),
            3
        )
        XCTAssertEqual(
            standardEnumValueName(enumID: 2, valueID: 3),
            "destructive"
        )
        XCTAssertNil(
            resolveStandardEnumValue(enumName: "ActionRole", valueName: "non_existent")
        )
        XCTAssertNil(
            resolveStandardEnumValue(enumName: "NonExistentEnum", valueName: "val")
        )

        // Typed enum bridging
        let typedDestructive = StandardActionRole.destructive
        XCTAssertEqual(typedDestructive.enumToken, EnumToken.actionRoleDestructive)
        XCTAssertEqual(StandardActionRole(enumToken: EnumToken.actionRoleDestructive), .destructive)
        XCTAssertNil(StandardActionRole(enumToken: EnumToken(enumID: 999, valueID: 1)))
    }

    // MARK: - Architectural Conformance Check (§1)

    func testZeroAppKitImportsInSemanticModel() throws {
        var current = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        var semanticModelDir: URL?
        while current.path != "/" {
            let candidate = current.appendingPathComponent("client-macos/SemanticModel")
            if FileManager.default.fileExists(atPath: candidate.path) {
                semanticModelDir = candidate
                break
            }
            current = current.deletingLastPathComponent()
        }

        guard let dir = semanticModelDir else {
            XCTFail("Could not locate client-macos/SemanticModel directory")
            return
        }

        let fileManager = FileManager.default
        let files = try fileManager.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
        let swiftFiles = files.filter { $0.pathExtension == "swift" }
        XCTAssertFalse(swiftFiles.isEmpty, "SemanticModel directory should contain Swift files")

        for fileURL in swiftFiles {
            let content = try String(contentsOf: fileURL, encoding: .utf8)
            let lines = content.components(separatedBy: .newlines)
            for (lineNum, line) in lines.enumerated() {
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                if trimmed.starts(with: "//") { continue }
                if trimmed.contains("import AppKit") || trimmed.contains("import Cocoa") {
                    XCTFail("Forbidden AppKit/Cocoa import found in \(fileURL.lastPathComponent):\(lineNum + 1): '\(line)'")
                }
            }
        }
    }
}

private extension Array {
    var len_count: Int { count }
}
