//
// ExtensionMountResolver.swift
// RendererAppKit
//
// Exact extension negotiation, native/fallback mount classification, and §11.1 validation.
//

import SemanticModel

/// Fail-closed errors for extension content that cannot be mounted natively or through §11.1.
public enum ExtensionMountError: Error, Equatable, Sendable, CustomStringConvertible {
    case unsupportedExtensionWithoutFallback(TypeRef)
    case invalidExtensionFallback(TypeRef)

    public var description: String {
        switch self {
        case .unsupportedExtensionWithoutFallback(let typeRef):
            return "unsupported extension \(typeRef) has no standard fallback (§11.1)"
        case .invalidExtensionFallback(let typeRef):
            return "unsupported extension \(typeRef) has an invalid standard fallback (§11.1)"
        }
    }
}

/// Extension node kinds resolved from `ServerWelcome.extension_namespaces` (§15, §21).
public enum ExtensionControlKind: Equatable, Sendable {
    case terminal
}

/// One classification shared by extension validation, native control creation, and tree mounting.
public enum ExtensionMountDecision: Equatable, Sendable {
    case standard
    case native(ExtensionControlKind)
    case fallback(root: NodeId)
    case rejected(ExtensionMountError)
}

/// Owns the exact negotiated extension registry and §11.1 fallback policy.
@MainActor
public final class ExtensionMountResolver {
    private var extensionKinds: [TypeRef: ExtensionControlKind] = [:]

    public init() {}

    public func register(typeRef: TypeRef, kind: ExtensionControlKind) throws {
        guard typeRef.namespaceID != 0 else {
            throw ControlFactoryError.unsupportedNodeType(typeRef)
        }
        if let existing = extensionKinds[typeRef], existing != kind {
            throw ControlFactoryError.unsupportedNodeType(typeRef)
        }
        extensionKinds[typeRef] = kind
    }

    public func reset() {
        extensionKinds.removeAll()
    }

    public func extensionKind(for typeRef: TypeRef) -> ExtensionControlKind? {
        extensionKinds[typeRef]
    }

    /// Resolves native rendering, standard fallback, or a fail-closed error exactly once.
    public func decision(
        for node: Node,
        in store: SemanticStore? = nil
    ) -> ExtensionMountDecision {
        if node.nodeType.isStandard {
            return .standard
        }
        if let kind = extensionKinds[node.nodeType] {
            return .native(kind)
        }
        guard !node.orderedChildren.isEmpty else {
            return .rejected(.unsupportedExtensionWithoutFallback(node.nodeType))
        }
        guard node.orderedChildren.count == 1,
              let store,
              let fallbackRoot = node.orderedChildren.first,
              let subtree = store.subtreeNodeIDs(rootedAt: fallbackRoot),
              subtree.allSatisfy({ nodeID in
                  store.getNode(nodeID)?.nodeType.isStandard == true
              }) else {
            return .rejected(.invalidExtensionFallback(node.nodeType))
        }
        return .fallback(root: fallbackRoot)
    }

    /// Preflights the portion of the tree the renderer would mount.
    ///
    /// Native extensions suppress their fallback descendants, so those descendants are not
    /// independently validated or mounted by a capable client (§11.1).
    public func validateMountableExtensions(in store: SemanticStore) throws {
        var pending = store.rootIDs
        while let nodeID = pending.popLast() {
            guard let node = store.getNode(nodeID) else { continue }
            switch decision(for: node, in: store) {
            case .standard, .fallback:
                pending.append(contentsOf: node.orderedChildren)
            case .native:
                continue
            case .rejected(let error):
                throw error
            }
        }
    }

    /// Returns whether an incremental update targets fallback content hidden by a native extension.
    public func isSuppressedFallbackNode(_ nodeID: NodeId, in store: SemanticStore) -> Bool {
        var parentID = store.getNode(nodeID)?.parentID
        while let currentID = parentID, let parent = store.getNode(currentID) {
            if extensionKinds[parent.nodeType] != nil {
                return true
            }
            parentID = parent.parentID
        }
        return false
    }
}
