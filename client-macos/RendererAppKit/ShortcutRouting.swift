// ShortcutRouting.swift
// RendererAppKit
//
// macOS mapping and local dispatch of the standard `shortcut` property (PX-016-G01).
// Implements: §4 inv. 3 (focus and IME stay local), §4 inv. 13 (a value this client cannot bind is
// a diagnostic, never a silent guess), §7.6/§7.7 (a Button or Toggle shortcut is the same
// target/action dispatch as a click, so the server receives the identical semantic event),
// §22.5 (editor and collection shortcuts move local focus only), §22.6 (no dispatch while the
// first responder holds marked text), §22.8/§22.9 (the platform-formatted shortcut is
// discoverable in the tooltip and accessibility help).
//
// RendererAppKit is the only AppKit-touching client layer; the grammar, admission and reserved
// table are platform-neutral data in SemanticModel/Shortcut.swift.
import AppKit
import SemanticModel

/// The native key equivalent a semantic shortcut maps to on macOS: `primary` is Command.
public struct AppKitShortcutBinding: Equatable, Sendable {
    public let shortcut: Shortcut
    /// The key equivalent, as `NSMenuItem.keyEquivalent`/`NSButton.keyEquivalent` spell it.
    public let keyEquivalent: String
    /// Exactly the modifiers the event must carry (Command, Shift, Option).
    public let modifierMask: NSEvent.ModifierFlags
    /// Hardware key code for named keys, which are layout independent; `nil` for characters.
    public let keyCode: UInt16?
    /// Platform-formatted text, e.g. `⇧⌘R`.
    public let displayString: String

    /// Modifiers that participate in matching; Caps Lock, Fn and the numeric-pad flag never do.
    public static let relevantModifiers: NSEvent.ModifierFlags = [.command, .shift, .option, .control]

    public init(_ shortcut: Shortcut) {
        self.shortcut = shortcut
        var mask: NSEvent.ModifierFlags = []
        if shortcut.primary { mask.insert(.command) }
        if shortcut.shift { mask.insert(.shift) }
        if shortcut.alt { mask.insert(.option) }
        modifierMask = mask
        displayString = shortcut.displayString(for: .macOS)
        switch shortcut.key {
        case .character(let c):
            keyEquivalent = String(c)
            keyCode = nil
        case .named(let named):
            let (character, code) = Self.native(named)
            keyEquivalent = String(Character(UnicodeScalar(character)!))
            keyCode = code
        }
    }

    /// Whether a key-down `event` is exactly this shortcut.
    public func matches(_ event: NSEvent) -> Bool {
        guard event.type == .keyDown,
              event.modifierFlags.intersection(Self.relevantModifiers) == modifierMask else {
            return false
        }
        if let keyCode {
            return event.keyCode == keyCode
        }
        // Letters and digits follow the active layout, as menu key equivalents do. The
        // modifier-free character covers Shift-digit, whose shifted character is a symbol.
        let candidates = [
            event.characters(byApplyingModifiers: [])?.lowercased(),
            event.charactersIgnoringModifiers?.lowercased(),
        ]
        return candidates.contains(keyEquivalent)
    }

    private static func native(_ key: ShortcutNamedKey) -> (UInt32, UInt16) {
        switch key {
        case .return: return (0x0D, 36)
        case .escape: return (0x1B, 53)
        case .tab: return (0x09, 48)
        case .space: return (0x20, 49)
        case .delete: return (0x08, 51)
        case .up: return (UInt32(NSUpArrowFunctionKey), 126)
        case .down: return (UInt32(NSDownArrowFunctionKey), 125)
        case .left: return (UInt32(NSLeftArrowFunctionKey), 123)
        case .right: return (UInt32(NSRightArrowFunctionKey), 124)
        case .home: return (UInt32(NSHomeFunctionKey), 115)
        case .end: return (UInt32(NSEndFunctionKey), 119)
        case .pageUp: return (UInt32(NSPageUpFunctionKey), 116)
        case .pageDown: return (UInt32(NSPageDownFunctionKey), 121)
        case .function(let n):
            let codes: [UInt16] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111]
            return (UInt32(NSF1FunctionKey) + UInt32(n - 1), codes[n - 1])
        }
    }
}

/// Why a node's `shortcut` is not bound on this client. Held on the node's `RenderHandle`, so its
/// lifetime is the handle's, and logged through `RendererDiagnostics`.
public enum ShortcutDiagnostic: Equatable, Sendable {
    /// The combination is in the macOS reserved table and stays with the system or the shell.
    case reserved(nodeID: NodeId, shortcut: String, reservedName: String)
    /// The value is not in the grammar. The store refuses this, so it signals a store bypass.
    case invalid(nodeID: NodeId, reason: String)
    /// The node type does not admit a shortcut. Also refused by the store.
    case notAdmitted(nodeID: NodeId, nodeType: TypeRef)
}

/// Why a matched shortcut was not dispatched.
public enum ShortcutDispatchRefusal: Equatable, Sendable {
    case windowNotKey
    case markedText
    case disabled
    case hidden
    case noNativeTarget
    /// More than one bound node in the window matches the event. The stores refuse duplicates
    /// within a window-owning Surface, so this signals a replica bypass; nothing is dispatched
    /// rather than silently picking one (§4 inv. 13).
    case ambiguous([NodeId])
}

/// Routes key equivalents of one renderer's Surface windows to the semantic node bound to them.
///
/// Every Surface window's content view asks this router first (`SurfaceContentView`). A match is
/// dispatched only when the window is key, the first responder holds no marked text, and the
/// node and its ancestors are enabled and visible in the committed semantic state, and exactly
/// one bound node in the window matches (two would be `.ambiguous`, never a silent pick). A refused
/// match returns `false`, so the event continues to the main menu and the first responder
/// unchanged.
@MainActor
public final class ShortcutRouter {
    private let registry: RenderRegistry
    /// The committed semantic state the guards read; updated on every mount and apply.
    var store = SemanticStore()
    /// Whether `window` is the key window. A test seam: test hosts never activate (§22).
    public var isWindowKey: @MainActor (NSWindow) -> Bool = { $0.isKeyWindow }
    /// The most recent refusal, for diagnostics and tests; overwritten, never accumulated.
    public private(set) var lastRefusal: ShortcutDispatchRefusal?

    init(registry: RenderRegistry) {
        self.registry = registry
    }

    /// Every handle in `window` whose binding matches `event`, in NodeId order. The stores
    /// guarantee at most one per window-owning Surface.
    ///
    /// lc-debt: linear scan of every mounted handle per key equivalent (key events arrive at human
    /// rate, so this is cheap); a per-window binding table rebuilt on mount/apply would make it O(1).
    public func boundHandles(for event: NSEvent, in window: NSWindow) -> [RenderHandle] {
        registry.allHandles
            .filter { handle in
                guard let binding = handle.shortcutBinding, binding.matches(event) else { return false }
                return handle.view.window === window
            }
            .sorted { $0.nodeID < $1.nodeID }
    }

    /// Handles a key equivalent for `window`; `true` when a shortcut was dispatched.
    public func handleKeyEquivalent(_ event: NSEvent, in window: NSWindow) -> Bool {
        let matches = boundHandles(for: event, in: window)
        guard let handle = matches.first else { return false }
        guard matches.count == 1 else {
            let ids = matches.map(\.nodeID)
            lastRefusal = .ambiguous(ids)
            RendererDiagnostics.log(
                "shortcut matches \(ids.count) nodes \(ids) in one window; none dispatched"
            )
            return false
        }
        if let refusal = refusal(for: handle, in: window) {
            lastRefusal = refusal
            RendererDiagnostics.log("shortcut node=\(handle.nodeID) not dispatched: \(refusal)")
            return false
        }
        lastRefusal = nil
        return dispatch(handle, in: window)
    }

    private func refusal(for handle: RenderHandle, in window: NSWindow) -> ShortcutDispatchRefusal? {
        guard isWindowKey(window) else { return .windowNotKey }
        if let client = window.firstResponder as? NSTextInputClient, client.hasMarkedText() {
            return .markedText
        }
        guard let node = store.getNode(handle.nodeID) else { return .noNativeTarget }
        if node.getProperty(.enabled)?.asBool == false { return .disabled }
        var current: Node? = node
        while let checked = current {
            switch checked.getProperty(.visibility)?.asEnumToken {
            case .visibilityHidden?, .visibilityCollapsed?: return .hidden
            default: break
            }
            current = checked.parentID.flatMap { store.getNode($0) }
        }
        return nil
    }

    private func dispatch(_ handle: RenderHandle, in window: NSWindow) -> Bool {
        switch ShortcutBehavior(nodeType: handle.nodeType) {
        case .activate?:
            // The same target/action a click sends, so the session builds the identical ACTIVATE.
            guard let control = handle.view as? NSControl, control.isEnabled,
                  let action = control.action else {
                lastRefusal = .noNativeTarget
                return false
            }
            return control.sendAction(action, to: control.target)
        case .toggle?:
            // A click flips the checkbox, then sends its action; the trampoline reads the new state.
            guard let button = handle.view as? NSButton, button.isEnabled,
                  let action = button.action else {
                lastRefusal = .noNativeTarget
                return false
            }
            button.state = button.state == .on ? .off : .on
            return button.sendAction(action, to: button.target)
        case .focus?:
            guard let target = Self.focusTarget(of: handle) else {
                lastRefusal = .noNativeTarget
                return false
            }
            return window.makeFirstResponder(target)
        case nil:
            lastRefusal = .noNativeTarget
            return false
        }
    }

    /// The native view that takes focus for a focus-only shortcut.
    static func focusTarget(of handle: RenderHandle) -> NSView? {
        if let scroll = handle.view as? NSScrollView {
            return scroll.documentView
        }
        return handle.view
    }
}

/// A Surface window's content view: asks the renderer's `ShortcutRouter` before the view
/// hierarchy's own key equivalents, and again for key-downs no responder consumed.
@MainActor
final class SurfaceContentView: NSView {
    var keyEquivalentHandler: (@MainActor (NSEvent, NSWindow) -> Bool)?

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if let window, keyEquivalentHandler?(event, window) == true {
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    override func keyDown(with event: NSEvent) {
        if let window, keyEquivalentHandler?(event, window) == true {
            return
        }
        super.keyDown(with: event)
    }
}

extension ControlFactory {
    /// Applies the standard `shortcut` property: binding, diagnostics and discoverability.
    func applyShortcut(_ value: Value?, to handle: RenderHandle) {
        handle.shortcutBinding = nil
        handle.shortcutDiagnostic = nil
        handle.accessibilityMetadata.shortcut = nil
        defer { refreshShortcutDiscoverability(of: handle) }
        guard let value else { return }
        let shortcut: Shortcut
        do {
            shortcut = try Shortcut(value: value)
        } catch {
            handle.shortcutDiagnostic = .invalid(nodeID: handle.nodeID, reason: "\(error)")
            RendererDiagnostics.log("shortcut node=\(handle.nodeID) invalid: \(error)")
            return
        }
        guard ShortcutBehavior(nodeType: handle.nodeType) != nil else {
            handle.shortcutDiagnostic = .notAdmitted(nodeID: handle.nodeID, nodeType: handle.nodeType)
            RendererDiagnostics.log("shortcut node=\(handle.nodeID) not admitted on \(handle.nodeType)")
            return
        }
        if let reserved = shortcut.reservedEntry(on: .macOS) {
            handle.shortcutDiagnostic = .reserved(
                nodeID: handle.nodeID,
                shortcut: shortcut.canonical,
                reservedName: reserved.name
            )
            RendererDiagnostics.log(
                "shortcut node=\(handle.nodeID) \(shortcut.canonical) is reserved (\(reserved.name)); not bound"
            )
            return
        }
        let binding = AppKitShortcutBinding(shortcut)
        handle.shortcutBinding = binding
        handle.accessibilityMetadata.shortcut = binding.displayString
    }

    /// Tooltip and accessibility help carry the bound shortcut (§22.8, §22.9). Without one, help is
    /// exactly `accessible_description` and no tooltip is set, as before shortcuts existed.
    func refreshShortcutDiscoverability(of handle: RenderHandle) {
        let metadata = handle.accessibilityMetadata
        let help: String?
        if let shortcut = metadata.shortcut {
            let hint = "Keyboard shortcut \(shortcut)"
            help = metadata.description.map { "\($0). \(hint)" } ?? hint
            let name = metadata.label ?? metadata.description
            handle.view.toolTip = name.map { "\($0) (\(shortcut))" } ?? shortcut
        } else {
            help = metadata.description
            handle.view.toolTip = nil
        }
        handle.view.setAccessibilityHelp(help)
        if let inner = ShortcutRouter.focusTarget(of: handle), inner !== handle.view {
            // The focused editor or collection announces the shortcut too; it carried no help
            // before shortcuts existed, so it carries none without one.
            inner.setAccessibilityHelp(metadata.shortcut == nil ? nil : help)
        }
    }
}
