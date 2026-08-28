import SemanticModel
import Testing
@testable import RendererAppKit

struct DirtyClassifierTests {
    @Test
    func scalarSetPropertyIsContentOnly() {
        let operation = Operation.setProperty(
            id: 912,
            property: .value,
            value: .float64(0.72)
        )

        #expect(
            DirtyClassifier.classify(operation)
                == [.contentOnly(nodeID: 912, property: .value)]
        )
    }

    @Test(arguments: [
        Operation.createNode(id: 1, nodeType: .text),
        Operation.deleteNode(id: 1),
    ])
    func createAndDeleteAreStructureAffecting(operation: Operation) {
        #expect(
            DirtyClassifier.classify(operation)
                == [.structureAffecting(operation: operation)]
        )
    }

    @Test
    func representativePropertiesUseMinimalDirtyCategories() {
        #expect(
            DirtyClassifier.classify(
                .setProperty(id: 1, property: .role, value: .enumToken(.textRoleBody))
            ) == [.appearanceRole(nodeID: 1, property: .role)]
        )
        #expect(
            DirtyClassifier.classify(
                .setProperty(id: 1, property: .preferredSize, value: .size(Size(width: 20, height: 10)))
            ) == [.layoutAffecting(nodeID: 1, property: .preferredSize)]
        )
        #expect(
            DirtyClassifier.classify(
                .setProperty(id: 1, property: .accessibleDescription, value: .string("Help"))
            ) == [.accessibilityOnly(nodeID: 1, property: .accessibleDescription)]
        )
        #expect(
            DirtyClassifier.classify(
                .clearProperty(id: 1, property: .resource)
            ) == [.resourceArrival(nodeID: 1, property: .resource)]
        )
    }
}
