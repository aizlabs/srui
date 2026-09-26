// SurfacePresentation conformance: a host process that has not declared a foreground activation
// policy must never paint a surface where a user can see it (§22, §22.3).
import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

@MainActor
struct SurfacePresentationTests {
    /// Every window this process currently has on screen, as the window server sees it —
    /// independent of anything the renderer holds. Returns `(windowNumber, alpha)` pairs.
    private static func onScreenWindowsOwnedByThisProcess() -> [(number: Int, alpha: Double)] {
        let pid = ProcessInfo.processInfo.processIdentifier
        let info = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
        ) as? [[String: Any]] ?? []
        return info.compactMap { entry in
            guard let owner = entry[kCGWindowOwnerPID as String] as? Int, Int32(owner) == pid,
                  let number = entry[kCGWindowNumber as String] as? Int else { return nil }
            return (number, (entry[kCGWindowAlpha as String] as? Double) ?? 1)
        }
    }

    @Test
    func testHostHasNotDeclaredAForegroundPolicy() {
        // The premise of the guard: swift-test hosts keep AppKit's default `.prohibited`.
        #expect(NSApplication.shared.activationPolicy() == .prohibited)
        #expect(SurfacePresentation.forHostApplication() == .concealed)
    }

    /// Both directions of the policy mapping, so nobody can quietly flip the default:
    /// an app that declares it presents UI keeps presenting it.
    @Test
    func policyMappingRunsBothWays() {
        #expect(SurfacePresentation.forActivationPolicy(.regular) == .onScreen)
        #expect(SurfacePresentation.forActivationPolicy(.accessory) == .onScreen)
        #expect(SurfacePresentation.forActivationPolicy(.prohibited) == .concealed)
    }

    /// The `.onScreen` branch must order the surface in and conceal nothing.
    ///
    /// The window is pre-concealed by the test itself (not by the policy) so that exercising
    /// the foreground branch inside a `.prohibited` test host still paints nothing on the
    /// developer's display; what is asserted is that the policy leaves alpha and mouse
    /// handling alone and brings the window into the window list.
    @Test
    func onScreenPresentationOrdersTheSurfaceInAndConcealsNothing() throws {
        let handle = try ControlFactory().makeHandle(for: Node(id: 1, nodeType: .surface))
        let window = try #require(handle.window)
        window.alphaValue = 0
        defer { window.orderOut(nil); window.close() }

        SurfacePresentation.onScreen.present(window)

        #expect(window.isVisible)
        #expect(window.alphaValue == 0, "the foreground branch must not touch alpha")
        #expect(window.ignoresMouseEvents == false, "the foreground branch must stay interactive")
    }

    private func makeSurfaceStore() throws -> SemanticStore {
        var store = SemanticStore()
        try SemanticModel.Operation.createNode(id: 1, nodeType: .surface).apply(to: &store)
        return store
    }

    @Test
    func showWindowsKeepsSurfacesOffTheDisplayWithoutHidingThemFromTheRenderer() throws {
        let store = try makeSurfaceStore()
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        renderer.showWindows()
        let window = try #require(renderer.registry.surfaceHandles.first?.window)
        defer { window.orderOut(nil); window.close() }

        // Still a real, ordered-in, laid-out, drawable window: every native assertion other
        // tests make about surfaces keeps holding.
        #expect(window.isVisible)
        #expect(window.contentView != nil)
        window.contentView?.layoutSubtreeIfNeeded()
        window.displayIfNeeded()
        #expect(window.contentView?.bitmapImageRepForCachingDisplay(in: window.contentView!.bounds) != nil)

        // But nothing is painted where a user could see it, and it cannot swallow clicks.
        #expect(window.alphaValue == 0)
        #expect(window.ignoresMouseEvents)
        #expect(window.isKeyWindow == false)
        #expect(NSApplication.shared.isActive == false)

        // Confirmed through the window server rather than the renderer's own state: window
        // registration is asynchronous, so give it run-loop turns to appear before asserting
        // that nothing this process has on screen is drawn at all.
        let deadline = Date().addingTimeInterval(2)
        while Date() < deadline,
              !Self.onScreenWindowsOwnedByThisProcess().contains(where: { $0.number == window.windowNumber }) {
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }
        let onScreen = Self.onScreenWindowsOwnedByThisProcess()
        #expect(onScreen.allSatisfy { $0.alpha == 0 },
                "this process must paint nothing on the display: \(onScreen)")
    }

    @Test
    func concealedPresentationNeverOrdersASurfaceFront() throws {
        let handle = try ControlFactory().makeHandle(for: Node(id: 1, nodeType: .surface))
        let window = try #require(handle.window)
        defer { window.orderOut(nil); window.close() }
        SurfacePresentation.concealed.present(window)
        #expect(window.isVisible)
        #expect(window.alphaValue == 0)
        #expect(window.ignoresMouseEvents)
    }
}
