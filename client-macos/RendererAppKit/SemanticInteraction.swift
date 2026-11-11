import Foundation
import SemanticModel

/// User interaction intent routed from native AppKit controls to the semantic event system (§7.6, §7.7, §22).
public enum SemanticInteraction: Equatable, Sendable {
    case activate(nodeID: NodeId)
    case valueChanged(nodeID: NodeId, value: Value)
    case selectionChanged(nodeID: NodeId, itemID: ItemId)
    /// A user action left a List, Table or Tree with no selected item: `SELECTION_CHANGED` with no
    /// `value` (§7.6; `spec/registries.md` §4.4). Never sent for a server-driven deselection.
    case selectionCleared(nodeID: NodeId)
    /// A Tree item's local disclosure changed (§7.6, §22.5). Carries the ItemId, never a row.
    case expansionChanged(nodeID: NodeId, itemID: ItemId, expanded: Bool)
    case textEdit(nodeID: NodeId, text: String, editSeq: EditSeq, laneEpoch: UInt64)
}
