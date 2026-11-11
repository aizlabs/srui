import Foundation
import SemanticModel

/// User interaction intent routed from native AppKit controls to the semantic event system (§7.6, §7.7, §22).
public enum SemanticInteraction: Equatable, Sendable {
    case activate(nodeID: NodeId)
    case valueChanged(nodeID: NodeId, value: Value)
    case selectionChanged(nodeID: NodeId, itemID: ItemId)
    /// A Tree item's local disclosure changed (§7.6, §22.5). Carries the ItemId, never a row.
    case expansionChanged(nodeID: NodeId, itemID: ItemId, expanded: Bool)
    case textEdit(nodeID: NodeId, text: String, editSeq: EditSeq, laneEpoch: UInt64)
}
