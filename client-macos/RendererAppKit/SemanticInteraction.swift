import Foundation
import SemanticModel

/// User interaction intent routed from native AppKit controls to the semantic event system (§7.6, §7.7, §22).
public enum SemanticInteraction: Equatable, Sendable {
    case activate(nodeID: NodeId)
    case valueChanged(nodeID: NodeId, value: Value)
    case selectionChanged(nodeID: NodeId, itemID: ItemId)
    case textEdit(nodeID: NodeId, text: String, editSeq: EditSeq)
}
