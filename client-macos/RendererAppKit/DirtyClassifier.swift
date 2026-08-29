import SemanticModel

public enum DirtyClassification: Equatable, Sendable {
    case contentOnly(nodeID: NodeId, property: PropertyRef)
    case appearanceRole(nodeID: NodeId, property: PropertyRef)
    case layoutAffecting(nodeID: NodeId, property: PropertyRef)
    case accessibilityOnly(nodeID: NodeId, property: PropertyRef)
    case resourceArrival(nodeID: NodeId, property: PropertyRef)
    case modelContent(modelID: ModelId)
    case structureAffecting(operation: Operation)

    public var isStructureAffecting: Bool {
        if case .structureAffecting = self { return true }
        return false
    }
}

/// Conservative §23 dirty classification. Scalar node properties and model content mutations
/// remain eligible for in-place mutation; graph mutations and model creation take the structural path.
public enum DirtyClassifier {
    public static func classify(_ operation: Operation) -> [DirtyClassification] {
        switch operation {
        case .setProperty(let nodeID, let property, _),
             .clearProperty(let nodeID, let property):
            return [classify(nodeID: nodeID, property: property)]

        case .batchPropertySet(let nodeID, let properties):
            return properties.map {
                classify(nodeID: nodeID, property: $0.property)
            }

        case .modelInsert(let id, _, _),
             .modelDelete(let id, _, _, _),
             .modelUpdate(let id, _, _),
             .modelResetRange(let id, _, _, _):
            return [.modelContent(modelID: id)]

        case .createNode,
             .deleteNode,
             .moveNode,
             .reorderChildren,
             .createModel:
            return [.structureAffecting(operation: operation)]
        }
    }

    public static func classify(_ transaction: Transaction) -> [DirtyClassification] {
        transaction.operations.flatMap(classify)
    }

    private static func classify(
        nodeID: NodeId,
        property: PropertyRef
    ) -> DirtyClassification {
        switch property {
        case .role, .presentationHint, .validationState:
            return .appearanceRole(nodeID: nodeID, property: property)

        case .visibility,
             .horizontalAlignment,
             .verticalAlignment,
             .grow,
             .shrink,
             .minimumSize,
             .maximumSize,
             .preferredSize,
             .spacingRole,
             .paddingRole:
            return .layoutAffecting(nodeID: nodeID, property: property)

        case .accessibleDescription, .valueDescription, .actions:
            return .accessibilityOnly(nodeID: nodeID, property: property)

        case .resource:
            return .resourceArrival(nodeID: nodeID, property: property)

        default:
            return .contentOnly(nodeID: nodeID, property: property)
        }
    }
}
