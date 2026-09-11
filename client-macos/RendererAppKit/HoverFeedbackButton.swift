import AppKit

/// Native button whose pointer feedback is owned entirely by the local renderer (§22.5).
@_spi(Benchmark)
@MainActor
public final class HoverFeedbackButton: NSButton {
    @_spi(Benchmark)
    public typealias ScreenPointerLocationProvider = @MainActor () -> NSPoint
    @_spi(Benchmark)
    public typealias ApplicationActiveProvider = @MainActor () -> Bool
    @_spi(Benchmark)
    public typealias WindowVisibilityProvider = @MainActor (NSWindow) -> Bool

    @_spi(Benchmark)
    public var screenPointerLocationProvider: ScreenPointerLocationProvider = {
        NSEvent.mouseLocation
    }
    @_spi(Benchmark)
    public var applicationActiveProvider: ApplicationActiveProvider = {
        NSApplication.shared.isActive
    }
    @_spi(Benchmark)
    public var windowVisibilityProvider: WindowVisibilityProvider = { window in
        window.isVisible
            && window.isMiniaturized == false
            && window.occlusionState.contains(.visible)
    }

    private var feedbackTrackingArea: NSTrackingArea?
    private weak var observedWindow: NSWindow?
    private var observesApplicationState = false
    @_spi(Benchmark)
    public private(set) var isPointerInside = false

    private static let windowContextNotifications: [Notification.Name] = [
        NSWindow.didMoveNotification,
        NSWindow.didResizeNotification,
        NSWindow.didMiniaturizeNotification,
        NSWindow.didDeminiaturizeNotification,
        NSWindow.didChangeOcclusionStateNotification,
    ]

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        installApplicationStateObserversIfNeeded()
        observeCurrentWindow()
        reconcilePointerState()
    }

    public override func updateTrackingAreas() {
        if let feedbackTrackingArea {
            removeTrackingArea(feedbackTrackingArea)
        }
        super.updateTrackingAreas()
        let trackingArea = NSTrackingArea(
            rect: .zero,
            options: [.mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(trackingArea)
        feedbackTrackingArea = trackingArea
        reconcilePointerState()
    }

    public override func mouseEntered(with event: NSEvent) {
        super.mouseEntered(with: event)
        reconcilePointerState()
    }

    public override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        reconcilePointerState()
    }

    public override func draw(_ dirtyRect: NSRect) {
        reconcilePointerState(scheduleDisplay: false)
        super.draw(dirtyRect)
        guard isPointerInside, isEnabled else { return }
        let overlayRect = bounds.insetBy(dx: 1, dy: 1)
        guard overlayRect.isEmpty == false else { return }
        NSColor.controlAccentColor.withAlphaComponent(0.10).setFill()
        NSBezierPath(
            roundedRect: overlayRect,
            xRadius: 6,
            yRadius: 6
        ).fill()
    }

    /// Recomputes hover from current process state instead of trusting a possibly missed exit.
    @_spi(Benchmark)
    @discardableResult
    public func reconcilePointerState(scheduleDisplay: Bool = true) -> Bool {
        let pointerInside = currentPointerIsInside
        guard pointerInside != isPointerInside else { return pointerInside }
        isPointerInside = pointerInside
        if scheduleDisplay {
            needsDisplay = true
        }
        return pointerInside
    }

    private var currentPointerIsInside: Bool {
        guard applicationActiveProvider(),
              let window,
              windowVisibilityProvider(window),
              isHiddenOrHasHiddenAncestor == false else {
            return false
        }
        let pointInWindow = window.convertPoint(fromScreen: screenPointerLocationProvider())
        return bounds.contains(convert(pointInWindow, from: nil))
    }

    private func installApplicationStateObserversIfNeeded() {
        guard observesApplicationState == false else { return }
        observesApplicationState = true
        let center = NotificationCenter.default
        center.addObserver(
            self,
            selector: #selector(pointerContextDidChange(_:)),
            name: NSApplication.didBecomeActiveNotification,
            object: NSApplication.shared
        )
        center.addObserver(
            self,
            selector: #selector(pointerContextDidChange(_:)),
            name: NSApplication.didResignActiveNotification,
            object: NSApplication.shared
        )
    }

    private func observeCurrentWindow() {
        let center = NotificationCenter.default
        if let observedWindow {
            for name in Self.windowContextNotifications {
                center.removeObserver(self, name: name, object: observedWindow)
            }
        }
        observedWindow = window
        guard let window else { return }
        for name in Self.windowContextNotifications {
            center.addObserver(
                self,
                selector: #selector(pointerContextDidChange(_:)),
                name: name,
                object: window
            )
        }
    }

    @objc private func pointerContextDidChange(_ notification: Notification) {
        reconcilePointerState()
    }
}
