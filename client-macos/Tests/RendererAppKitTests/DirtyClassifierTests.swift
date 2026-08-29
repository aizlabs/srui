import SemanticModel
import Testing
@testable import RendererAppKit

struct DirtyClassifierTests {
    enum ExpectedCategory: Equatable, Sendable {
        case contentOnly
        case appearanceRole
        case layoutAffecting
        case accessibilityOnly
        case resourceArrival
    }

    struct PropertyCase: Sendable {
        let property: PropertyRef
        let expected: ExpectedCategory
    }

    /// Every standard namespace-0 `PropertyRef` and its §23 dirty category.
    private static let allPropertyCases: [PropertyCase] = [
        PropertyCase(property: .label, expected: .contentOnly),
        PropertyCase(property: .accessibleDescription, expected: .accessibilityOnly),
        PropertyCase(property: .role, expected: .appearanceRole),
        PropertyCase(property: .valueDescription, expected: .accessibilityOnly),
        PropertyCase(property: .actions, expected: .accessibilityOnly),
        PropertyCase(property: .visibility, expected: .layoutAffecting),
        PropertyCase(property: .enabled, expected: .contentOnly),
        PropertyCase(property: .readOnly, expected: .contentOnly),
        PropertyCase(property: .busy, expected: .contentOnly),
        PropertyCase(property: .selected, expected: .contentOnly),
        PropertyCase(property: .validationState, expected: .appearanceRole),
        PropertyCase(property: .text, expected: .contentOnly),
        PropertyCase(property: .value, expected: .contentOnly),
        PropertyCase(property: .placeholder, expected: .contentOnly),
        PropertyCase(property: .resource, expected: .resourceArrival),
        PropertyCase(property: .items, expected: .contentOnly),
        PropertyCase(property: .modelRef, expected: .contentOnly),
        PropertyCase(property: .horizontalAlignment, expected: .layoutAffecting),
        PropertyCase(property: .verticalAlignment, expected: .layoutAffecting),
        PropertyCase(property: .grow, expected: .layoutAffecting),
        PropertyCase(property: .shrink, expected: .layoutAffecting),
        PropertyCase(property: .minimumSize, expected: .layoutAffecting),
        PropertyCase(property: .maximumSize, expected: .layoutAffecting),
        PropertyCase(property: .preferredSize, expected: .layoutAffecting),
        PropertyCase(property: .spacingRole, expected: .layoutAffecting),
        PropertyCase(property: .paddingRole, expected: .layoutAffecting),
        PropertyCase(property: .presentationHint, expected: .appearanceRole),
        PropertyCase(property: .actionKey, expected: .contentOnly),
        PropertyCase(property: .columns, expected: .contentOnly),
        PropertyCase(property: .selectionMode, expected: .contentOnly),
    ]

    private static let structuralOperations: [Operation] = [
        .createNode(id: 1, nodeType: .text),
        .deleteNode(id: 1),
        .moveNode(id: 2, newParentID: 1, newChildIndex: 0),
        .reorderChildren(parentID: 1, newOrder: [2, 3]),
        .createModel(id: ModelId(1), modelType: .list, itemCount: 0),
    ]

    private static let modelContentOperations: [(Operation, ModelId)] = [
        (
            .modelInsert(
                id: ModelId(1),
                index: 0,
                items: [ModelItem(itemID: ItemId(1), value: .string("item"))]
            ),
            ModelId(1)
        ),
        (
            .modelDelete(id: ModelId(2), index: 0, count: 1, itemIds: []),
            ModelId(2)
        ),
        (
            .modelUpdate(
                id: ModelId(3),
                index: 0,
                items: [ModelItem(itemID: ItemId(1), value: .string("updated"))]
            ),
            ModelId(3)
        ),
        (
            .modelResetRange(
                id: ModelId(4),
                startIndex: 0,
                items: [ModelItem(itemID: ItemId(2), value: .string("reset"))],
                totalCount: 1
            ),
            ModelId(4)
        ),
    ]

    private static let scalarOperations: [Operation] = [
        .setProperty(id: 912, property: .value, value: .float64(0.72)),
        .clearProperty(id: 7, property: .label),
        .batchPropertySet(
            id: 8,
            properties: [
                Property(property: .text, value: .string("hello")),
                Property(property: .enabled, value: .bool(true)),
            ]
        ),
    ]

    private static let unknownProperties: [PropertyRef] = [
        PropertyRef(namespaceID: 99, localID: 1),
        PropertyRef.standard(999),
    ]

    @Test(arguments: allPropertyCases)
    func setPropertyMapsEveryStandardProperty(case propertyCase: PropertyCase) {
        let nodeID: NodeId = 42
        let operation = Operation.setProperty(
            id: nodeID,
            property: propertyCase.property,
            value: .string("sample")
        )

        #expect(
            DirtyClassifier.classify(operation)
                == [Self.expectedClassification(
                    nodeID: nodeID,
                    property: propertyCase.property,
                    category: propertyCase.expected
                )]
        )
    }

    @Test(arguments: allPropertyCases)
    func clearPropertyMapsEveryStandardProperty(case propertyCase: PropertyCase) {
        let nodeID: NodeId = 43
        let operation = Operation.clearProperty(id: nodeID, property: propertyCase.property)

        #expect(
            DirtyClassifier.classify(operation)
                == [Self.expectedClassification(
                    nodeID: nodeID,
                    property: propertyCase.property,
                    category: propertyCase.expected
                )]
        )
    }

    @Test(arguments: structuralOperations)
    func structuralOperationsAreStructureAffecting(operation: Operation) {
        let classifications = DirtyClassifier.classify(operation)

        #expect(classifications == [.structureAffecting(operation: operation)])
        #expect(classifications.allSatisfy { $0.isStructureAffecting })
    }

    @Test(arguments: modelContentOperations)
    func modelContentOperationsClassifyAsModelContent(opCase: (Operation, ModelId)) {
        let classifications = DirtyClassifier.classify(opCase.0)

        #expect(classifications == [.modelContent(modelID: opCase.1)])
        #expect(!classifications.contains { $0.isStructureAffecting })
    }

    @Test(arguments: scalarOperations)
    func scalarOperationsAreNeverStructureAffecting(operation: Operation) {
        let classifications = DirtyClassifier.classify(operation)

        #expect(!classifications.contains { $0.isStructureAffecting })
    }

    @Test
    func batchPropertySetPreservesPropertyOrder() {
        let nodeID: NodeId = 55
        let properties = [
            Property(property: .value, value: .float64(1)),
            Property(property: .role, value: .enumToken(.textRoleBody)),
            Property(property: .preferredSize, value: .size(Size(width: 20, height: 10))),
            Property(property: .accessibleDescription, value: .string("Help")),
            Property(property: .resource, value: .string("res://icon")),
        ]
        let operation = Operation.batchPropertySet(id: nodeID, properties: properties)

        #expect(
            DirtyClassifier.classify(operation)
                == [
                    .contentOnly(nodeID: nodeID, property: .value),
                    .appearanceRole(nodeID: nodeID, property: .role),
                    .layoutAffecting(nodeID: nodeID, property: .preferredSize),
                    .accessibilityOnly(nodeID: nodeID, property: .accessibleDescription),
                    .resourceArrival(nodeID: nodeID, property: .resource),
                ]
        )
    }

    @Test
    func scalarOnlyTransactionNeverRequiresRemount() {
        let transaction = Transaction(
            baseRevision: 0,
            operations: [
                .setProperty(id: 1, property: .text, value: .string("alpha")),
                .clearProperty(id: 2, property: .label),
                .batchPropertySet(
                    id: 3,
                    properties: [
                        Property(property: .value, value: .float64(2)),
                        Property(property: .enabled, value: .bool(false)),
                    ]
                ),
            ]
        )

        let classifications = DirtyClassifier.classify(transaction)

        #expect(!classifications.contains { $0.isStructureAffecting })
        #expect(classifications.count == 4)
    }

    @Test(arguments: structuralOperations)
    func anyStructuralOperationForcesRemount(operation: Operation) {
        let transaction = Transaction(
            baseRevision: 1,
            operations: [
                .setProperty(id: 1, property: .text, value: .string("before")),
                operation,
                .setProperty(id: 2, property: .value, value: .float64(3)),
            ]
        )

        let classifications = DirtyClassifier.classify(transaction)

        #expect(classifications.contains { $0.isStructureAffecting })
        #expect(
            classifications.filter { $0.isStructureAffecting }
                == [.structureAffecting(operation: operation)]
        )
    }

    @Test(arguments: unknownProperties)
    func unknownPropertyUsesConservativeContentOnlyPath(property: PropertyRef) {
        let nodeID: NodeId = 99

        #expect(
            DirtyClassifier.classify(.setProperty(id: nodeID, property: property, value: .string("x")))
                == [.contentOnly(nodeID: nodeID, property: property)]
        )
        #expect(
            DirtyClassifier.classify(.clearProperty(id: nodeID, property: property))
                == [.contentOnly(nodeID: nodeID, property: property)]
        )
    }

    private static func expectedClassification(
        nodeID: NodeId,
        property: PropertyRef,
        category: ExpectedCategory
    ) -> DirtyClassification {
        switch category {
        case .contentOnly:
            return .contentOnly(nodeID: nodeID, property: property)
        case .appearanceRole:
            return .appearanceRole(nodeID: nodeID, property: property)
        case .layoutAffecting:
            return .layoutAffecting(nodeID: nodeID, property: property)
        case .accessibilityOnly:
            return .accessibilityOnly(nodeID: nodeID, property: property)
        case .resourceArrival:
            return .resourceArrival(nodeID: nodeID, property: property)
        }
    }
}
