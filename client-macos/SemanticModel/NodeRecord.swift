//
// NodeRecord.swift
// SemanticModel
//
// Generic in-memory node record representing node identity, type, hierarchy, and properties (§6.2, §16).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//

import Foundation

/// Generic in-memory node record representing node identity, type, hierarchy, and properties (§6.2, §16).
public struct NodeRecord: Equatable, Sendable, CustomStringConvertible {
    /// Unique identifier for this node within the session (§6.2).
    public var nodeId: NodeId
    /// Type reference in a namespace registry (§6.4, §7.2).
    public var nodeType: TypeRef
    /// Parent node ID in the hierarchy (`nil` for root nodes).
    public var parentId: NodeId?
    /// Child insertion index under parent (`nil` represents append / default placement).
    public var childIndex: Int?
    /// Defined property values on this node (§7.4).
    public var properties: [Property]

    public init(
        nodeId: NodeId,
        nodeType: TypeRef,
        parentId: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [Property] = []
    ) {
        self.nodeId = nodeId
        self.nodeType = nodeType
        self.parentId = parentId
        self.childIndex = childIndex
        self.properties = properties
    }

    public init(
        nodeId: NodeId,
        nodeType: TypeRef,
        parentId: NodeId? = nil,
        childIndex: Int? = nil,
        properties: [(PropertyRef, Value)]
    ) {
        self.nodeId = nodeId
        self.nodeType = nodeType
        self.parentId = parentId
        self.childIndex = childIndex
        self.properties = properties.map { Property(property: $0.0, value: $0.1) }
    }

    public init(from node: Node, childIndex: Int? = nil) {
        self.nodeId = node.id
        self.nodeType = node.nodeType
        self.parentId = node.parentID
        self.childIndex = childIndex
        self.properties = node.properties.map { Property(property: $0.key, value: $0.value) }
    }

    public func getProperty(_ prop: PropertyRef) -> Value? {
        properties.first(where: { $0.property == prop })?.value
    }

    public func hasProperty(_ prop: PropertyRef) -> Bool {
        properties.contains(where: { $0.property == prop })
    }

    public var description: String {
        "NodeRecord(id: \(nodeId), type: \(nodeType), parent: \(String(describing: parentId)), index: \(String(describing: childIndex)), props: \(properties.count))"
    }
}
