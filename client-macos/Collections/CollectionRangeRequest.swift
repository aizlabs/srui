//
// CollectionRangeRequest.swift
// Collections
//
// Primitive cache-miss request emitted by adapters. Session adds `observed_revision`
// and encodes `ClientModelRangeRequest` on the `.ui` lane (§8, §22.7).
//

import SemanticModel

/// A contiguous missing window of a sparse collection model (§8, §22.7).
public struct CollectionRangeRequest: Equatable, Sendable, Hashable {
    public let nodeID: NodeId
    public let modelID: ModelId
    public let startIndex: UInt64
    public let count: UInt64

    public init(nodeID: NodeId, modelID: ModelId, startIndex: UInt64, count: UInt64) {
        self.nodeID = nodeID
        self.modelID = modelID
        self.startIndex = startIndex
        self.count = count
    }
}
