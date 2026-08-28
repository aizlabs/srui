import AppKit
import SemanticModel

public enum RenderRegistryError: Error, Equatable, Sendable {
    case duplicateNodeID(NodeId)
}

/// Main-actor registry from semantic node identity to retained native representation.
@MainActor
public final class RenderRegistry {
    private var handles: [NodeId: RenderHandle] = [:]

    public init() {}

    public var count: Int {
        handles.count
    }

    public var isEmpty: Bool {
        handles.isEmpty
    }

    public var allHandles: [RenderHandle] {
        handles.values.sorted { $0.nodeID < $1.nodeID }
    }

    public var surfaceHandles: [RenderHandle] {
        allHandles.filter { $0.nodeType == .surface }
    }

    public func register(_ handle: RenderHandle) throws {
        guard handles[handle.nodeID] == nil else {
            throw RenderRegistryError.duplicateNodeID(handle.nodeID)
        }
        handles[handle.nodeID] = handle
    }

    public func handle(for nodeID: NodeId) -> RenderHandle? {
        handles[nodeID]
    }

    public func view(for nodeID: NodeId) -> NSView? {
        handles[nodeID]?.view
    }

    @discardableResult
    public func remove(_ nodeID: NodeId) -> RenderHandle? {
        handles.removeValue(forKey: nodeID)
    }

    /// Removes a retained subtree using the child identity metadata already held by render handles.
    @discardableResult
    public func removeSubtree(rootID: NodeId) -> [RenderHandle] {
        guard handles[rootID] != nil else { return [] }

        var pending = [rootID]
        var visited: Set<NodeId> = []
        var orderedIDs: [NodeId] = []
        while let nodeID = pending.popLast() {
            // Child metadata is not guaranteed acyclic, so visited tracking bounds the walk.
            guard let handle = handles[nodeID], visited.insert(nodeID).inserted else { continue }
            orderedIDs.append(nodeID)
            pending.append(contentsOf: handle.childIDs)
        }

        var removed: [RenderHandle] = []
        for nodeID in orderedIDs.reversed() {
            if let handle = handles.removeValue(forKey: nodeID) {
                removed.append(handle)
            }
        }
        return removed
    }

    @discardableResult
    public func removeAll() -> [RenderHandle] {
        let removed = allHandles
        handles.removeAll(keepingCapacity: true)
        return removed
    }
}
