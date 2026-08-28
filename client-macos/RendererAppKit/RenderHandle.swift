import AppKit
import SemanticModel

public struct RenderLayoutMetadata: Equatable, Sendable {
    public internal(set) var minimumSize: Size?
    public internal(set) var maximumSize: Size?
    public internal(set) var preferredSize: Size?
    public internal(set) var horizontalAlignment: EnumToken?
    public internal(set) var verticalAlignment: EnumToken?

    public init(
        minimumSize: Size? = nil,
        maximumSize: Size? = nil,
        preferredSize: Size? = nil,
        horizontalAlignment: EnumToken? = nil,
        verticalAlignment: EnumToken? = nil
    ) {
        self.minimumSize = minimumSize
        self.maximumSize = maximumSize
        self.preferredSize = preferredSize
        self.horizontalAlignment = horizontalAlignment
        self.verticalAlignment = verticalAlignment
    }
}

public struct RenderAccessibilityMetadata: Equatable, Sendable {
    public internal(set) var label: String?
    public internal(set) var description: String?
    public internal(set) var valueDescription: String?
    public internal(set) var actions: [String]

    public init(
        label: String? = nil,
        description: String? = nil,
        valueDescription: String? = nil,
        actions: [String] = []
    ) {
        self.label = label
        self.description = description
        self.valueDescription = valueDescription
        self.actions = actions
    }
}

/// Retained AppKit representation associated with one semantic node (§22.3).
@MainActor
public final class RenderHandle {
    public let nodeID: NodeId
    public let nodeType: TypeRef
    public let view: NSView
    public let window: NSWindow?
    public internal(set) var parentID: NodeId?
    public internal(set) var childIDs: [NodeId]
    public internal(set) var layoutMetadata: RenderLayoutMetadata
    public internal(set) var accessibilityMetadata: RenderAccessibilityMetadata
    public internal(set) var modelAdapter: AnyObject?
    /// Content hash of the resource most recently referenced by this node (§14).
    public internal(set) var pendingResourceHash: ResourceHash?

    internal var propertyConstraints: [PropertyRef: [NSLayoutConstraint]] = [:]

    public init(
        nodeID: NodeId,
        nodeType: TypeRef,
        view: NSView,
        window: NSWindow? = nil,
        parentID: NodeId? = nil,
        childIDs: [NodeId] = [],
        layoutMetadata: RenderLayoutMetadata = RenderLayoutMetadata(),
        accessibilityMetadata: RenderAccessibilityMetadata = RenderAccessibilityMetadata(),
        modelAdapter: AnyObject? = nil
    ) {
        self.nodeID = nodeID
        self.nodeType = nodeType
        self.view = view
        self.window = window
        self.parentID = parentID
        self.childIDs = childIDs
        self.layoutMetadata = layoutMetadata
        self.accessibilityMetadata = accessibilityMetadata
        self.modelAdapter = modelAdapter
    }
}
