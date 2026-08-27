import XCTest
import Foundation
@testable import TransportSSH
@testable import Protocol
@testable import SemanticModel
@testable import Session
@testable import RendererAppKit
@testable import Collections
@testable import Text
@testable import Terminal
@testable import Resources
@testable import Accessibility

final class SRUITests: XCTestCase {
    private var conformanceVectorsDir: URL {
        let testFileURL = URL(fileURLWithPath: #filePath)
        let repoRoot = testFileURL
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // client-macos
            .deletingLastPathComponent() // repo root
        return repoRoot.appendingPathComponent("protocol/conformance-vectors")
    }

    func testDecodeGoldenNodeRecord() throws {
        let fileURL = conformanceVectorsDir.appendingPathComponent("golden_node_record.bin")
        let data = try Data(contentsOf: fileURL)
        XCTAssertFalse(data.isEmpty, "Fixture data must not be empty")

        let node = try SRUINodeRecord(serializedBytes: data)

        XCTAssertEqual(node.nodeID, 42)
        XCTAssertTrue(node.hasType)
        XCTAssertEqual(node.type.namespaceID, standardNamespaceID)
        XCTAssertEqual(node.type.localID, UInt32(Srui_Protocol_StandardNodeType.nodeTypeButton.rawValue))
        XCTAssertEqual(node.parentID, 1)
        XCTAssertEqual(node.childIndex, 0)
        XCTAssertEqual(node.properties.count, 3)

        // Property 0: Label = "Delete"
        let p0 = node.properties[0]
        XCTAssertEqual(p0.property.namespaceID, standardNamespaceID)
        XCTAssertEqual(p0.property.localID, UInt32(Srui_Protocol_StandardProperty.propertyLabel.rawValue))
        guard case .stringValue(let str) = p0.value.value else {
            XCTFail("Expected stringValue for property 0")
            return
        }
        XCTAssertEqual(str, "Delete")

        // Property 1: Role = ActionRole.destructive (enum_id=2, value_id=3)
        let p1 = node.properties[1]
        XCTAssertEqual(p1.property.namespaceID, standardNamespaceID)
        XCTAssertEqual(p1.property.localID, UInt32(Srui_Protocol_StandardProperty.propertyRole.rawValue))
        guard case .enumValue(let ev) = p1.value.value else {
            XCTFail("Expected enumValue for property 1")
            return
        }
        XCTAssertEqual(ev.enumID, 2)
        XCTAssertEqual(ev.valueID, UInt32(Srui_Protocol_ActionRole.destructive.rawValue))

        // Property 2: Enabled = true
        let p2 = node.properties[2]
        XCTAssertEqual(p2.property.namespaceID, standardNamespaceID)
        XCTAssertEqual(p2.property.localID, UInt32(Srui_Protocol_StandardProperty.propertyEnabled.rawValue))
        guard case .boolValue(let b) = p2.value.value else {
            XCTFail("Expected boolValue for property 2")
            return
        }
        XCTAssertEqual(b, true)
    }

    func testDecodeGoldenTransaction() throws {
        let fileURL = conformanceVectorsDir.appendingPathComponent("golden_transaction.bin")
        let data = try Data(contentsOf: fileURL)
        XCTAssertFalse(data.isEmpty, "Fixture data must not be empty")

        let tx = try SRUITransaction(serializedBytes: data)

        XCTAssertEqual(tx.baseRevision, 104)
        XCTAssertEqual(tx.newRevision, 105)
        XCTAssertEqual(tx.priority, 1)
        XCTAssertEqual(tx.operations.count, 3)

        // Op 0: CREATE_NODE (Text node with text "27 tests passed")
        guard case .createNode(let createOp) = tx.operations[0].op else {
            XCTFail("Expected createNode operation for op 0")
            return
        }
        let createdNode = createOp.node
        XCTAssertEqual(createdNode.nodeID, 19)
        XCTAssertEqual(createdNode.type.namespaceID, standardNamespaceID)
        XCTAssertEqual(createdNode.type.localID, UInt32(Srui_Protocol_StandardNodeType.nodeTypeText.rawValue))
        XCTAssertEqual(createdNode.parentID, 2)
        XCTAssertEqual(createdNode.childIndex, 3)
        XCTAssertEqual(createdNode.properties.count, 1)
        XCTAssertEqual(createdNode.properties[0].property.localID, UInt32(Srui_Protocol_StandardProperty.propertyText.rawValue))
        guard case .stringValue(let text) = createdNode.properties[0].value.value else {
            XCTFail("Expected stringValue for text property")
            return
        }
        XCTAssertEqual(text, "27 tests passed")

        // Op 1: SET_PROPERTY (node 4, Value = 0.71)
        guard case .setProperty(let setOp) = tx.operations[1].op else {
            XCTFail("Expected setProperty operation for op 1")
            return
        }
        XCTAssertEqual(setOp.nodeID, 4)
        XCTAssertEqual(setOp.property.namespaceID, standardNamespaceID)
        XCTAssertEqual(setOp.property.localID, UInt32(Srui_Protocol_StandardProperty.propertyValue.rawValue))
        guard case .floatValue(let f) = setOp.value.value else {
            XCTFail("Expected floatValue for setOp value")
            return
        }
        XCTAssertEqual(f, 0.71, accuracy: 0.0001)

        // Op 2: BATCH_PROPERTY_SET (node 19, MinimumSize = 120.0 x 24.0)
        guard case .batchPropertySet(let batchOp) = tx.operations[2].op else {
            XCTFail("Expected batchPropertySet operation for op 2")
            return
        }
        XCTAssertEqual(batchOp.nodeID, 19)
        XCTAssertEqual(batchOp.properties.count, 1)
        let batchProp = batchOp.properties[0]
        XCTAssertEqual(batchProp.property.namespaceID, standardNamespaceID)
        XCTAssertEqual(batchProp.property.localID, UInt32(Srui_Protocol_StandardProperty.propertyMinimumSize.rawValue))
        guard case .sizeValue(let size) = batchProp.value.value else {
            XCTFail("Expected sizeValue for batchProp")
            return
        }
        XCTAssertEqual(size.width, 120.0)
        XCTAssertEqual(size.height, 24.0)
    }
}
