// SurfacePresentation — whether this process may put surface windows on the user's display.
// Implements the presentation side of §22 (macOS rendering architecture) and §22.3 (a Surface
// maps to an `NSWindow` + root content view).
//
// RendererAppKit is the only AppKit-touching client layer; SemanticModel must never import AppKit.
import AppKit

/// How `LayoutRenderer.showWindows()` brings surface windows into the window list.
///
/// AppKit's activation policy is the process-wide declaration of whether a process presents UI:
/// an app that does sets `.regular` (or `.accessory` for an agent/menu-bar app) during startup,
/// before it creates windows. A process that never declares one keeps AppKit's default
/// `.prohibited`: test hosts (`xctest`, `swiftpm-testing-helper`), the SSH bridge, and any other
/// headless embedder of this renderer.
///
/// `.prohibited` suppresses the Dock tile and activation but *not* window display — ordering a
/// window front from such a process still hands it to the window server, which is why a test run
/// flashes real windows across the developer's desktop and leaves them sitting there when a test
/// process parks instead of exiting.
///
/// So a `.prohibited` host gets `concealed` surfaces: mounted, attached to a real `NSWindow`,
/// laid out, drawable, `isVisible`, and ordered into the window list — but fully transparent,
/// non-interactive, and ordered to the back, so nothing is painted where a user can see it.
/// Every renderer invariant stays observable: native identity, view counts, cell contents,
/// `layoutSubtreeIfNeeded()`, `displayIfNeeded()` and `cacheDisplay(in:to:)` are unaffected by a
/// window's alpha.
@MainActor
public enum SurfacePresentation: Equatable, Sendable {
    /// Order the surface front and make it key: the host app has declared that it presents UI.
    case onScreen
    /// Order the surface in, but transparent, non-interactive and behind everything else.
    case concealed

    /// The presentation implied by a declared activation policy.
    public static func forActivationPolicy(
        _ policy: NSApplication.ActivationPolicy
    ) -> SurfacePresentation {
        switch policy {
        case .regular, .accessory:
            return .onScreen
        case .prohibited:
            return .concealed
        @unknown default:
            return .concealed
        }
    }

    /// The presentation implied by `application`'s declared activation policy.
    public static func forHostApplication(
        _ application: NSApplication = NSApplication.shared
    ) -> SurfacePresentation {
        forActivationPolicy(application.activationPolicy())
    }

    /// Brings `window` into the window list under this policy.
    public func present(_ window: NSWindow) {
        switch self {
        case .onScreen:
            window.makeKeyAndOrderFront(nil)
        case .concealed:
            window.alphaValue = 0
            window.ignoresMouseEvents = true
            window.orderBack(nil)
        }
    }
}
