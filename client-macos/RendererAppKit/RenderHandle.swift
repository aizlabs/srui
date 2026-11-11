import AppKit
import SemanticModel
import Text

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
    /// Platform-formatted bound shortcut (e.g. `⇧⌘R`), shown in the tooltip and accessibility help
    /// (§22.8). `nil` when the node has no shortcut or its shortcut is not bound here.
    public internal(set) var shortcut: String?

    public init(
        label: String? = nil,
        description: String? = nil,
        valueDescription: String? = nil,
        actions: [String] = [],
        shortcut: String? = nil
    ) {
        self.label = label
        self.description = description
        self.valueDescription = valueDescription
        self.actions = actions
        self.shortcut = shortcut
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
    public internal(set) var actionTrampoline: AnyObject?
    /// Native text-editor observer, retained separately from collection adapters (§22.6).
    public internal(set) var textAdapter: NativeTextEditorAdapter?
    /// Content hash of the resource most recently referenced by this node (§14).
    public internal(set) var pendingResourceHash: ResourceHash?
    /// Native key equivalent of the node's `shortcut`, when bound (PX-016-G01; §7.7, §22.5).
    public internal(set) var shortcutBinding: AppKitShortcutBinding?
    /// Why the node's `shortcut` is not bound (reserved, invalid or not admitted), if it is not.
    public internal(set) var shortcutDiagnostic: ShortcutDiagnostic?

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
        modelAdapter: AnyObject? = nil,
        actionTrampoline: AnyObject? = nil,
        textAdapter: NativeTextEditorAdapter? = nil
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
        self.actionTrampoline = actionTrampoline
        self.textAdapter = textAdapter
    }
}
