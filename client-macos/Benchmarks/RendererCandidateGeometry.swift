import AppKit
import CryptoKit
import Darwin
import Foundation
import Protocol
import RendererAppKit
import Resources
import SemanticModel
import Session
import SwiftProtobuf
import Terminal
import TransportSSH
import WebKit

enum RendererCandidatePaintStage: String, CaseIterable {
    case firstPaint = "first_paint"
    case completePaint = "complete_paint"
}

struct RendererCandidateMetricDescriptor {
    let stage: RendererCandidatePaintStage
    let label: String
    let includesCompletionTransaction: Bool
}

let rendererCandidateMetricDescriptors = [
    RendererCandidateMetricDescriptor(
        stage: .firstPaint,
        label: "first useful subtree",
        includesCompletionTransaction: false
    ),
    RendererCandidateMetricDescriptor(
        stage: .completePaint,
        label: "complete representative state",
        includesCompletionTransaction: true
    ),
]

enum RendererCandidateLifecycleEvent: String, CaseIterable {
    case warmCompleted = "warm_completed"
    case resetCompleted = "reset_completed"
    case timingStarted = "timing_started"
    case hostAttached = "host_attached"
    case representationIngested = "representation_ingested"
    case displaySubmitted = "display_submitted"
}

@MainActor
final class RendererCandidateLifecycle {
    let candidate: String
    let metric: RendererCandidateMetricDescriptor
    private(set) var events = [RendererCandidateLifecycleEvent]()

    init(candidate: String, metric: RendererCandidateMetricDescriptor) {
        self.candidate = candidate
        self.metric = metric
    }

    func record(_ event: RendererCandidateLifecycleEvent) throws {
        guard events.contains(event) == false else {
            throw BenchmarkFailure.message(
                "\(candidate) \(metric.label) recorded duplicate lifecycle "
                    + "event \(event.rawValue)"
            )
        }
        events.append(event)
    }

    func validate() throws -> String {
        func index(_ event: RendererCandidateLifecycleEvent) throws -> Int {
            guard let index = events.firstIndex(of: event) else {
                throw BenchmarkFailure.message(
                    "\(candidate) \(metric.label) omitted lifecycle event "
                        + event.rawValue
                )
            }
            return index
        }

        let warm = try index(.warmCompleted)
        let reset = try index(.resetCompleted)
        let started = try index(.timingStarted)
        let host = try index(.hostAttached)
        let representation = try index(.representationIngested)
        let submitted = try index(.displaySubmitted)
        guard events.count == RendererCandidateLifecycleEvent.allCases.count,
              warm < reset,
              reset < started,
              started < host,
              started < representation,
              host < submitted,
              representation < submitted else {
            throw BenchmarkFailure.message(
                "\(candidate) \(metric.label) lifecycle crossed the shared "
                    + "timing boundary: "
                    + events.map(\.rawValue).joined(separator: " -> ")
            )
        }
        return "\(candidate)/\(metric.stage.rawValue):"
            + events.map(\.rawValue).joined(separator: "->")
    }
}

struct RendererCandidateMeasuredPaint<State> {
    let state: State
    let measurement: BenchmarkPassivePresentationMeasurement
    let lifecycleEvidence: String
}

@MainActor
func benchmarkMeasureRendererCandidatePaint<State>(
    lifecycle: RendererCandidateLifecycle,
    on screen: NSScreen,
    requiredContentChangeFromObservation:
        OnScreenPaintObservation? = nil,
    action:
        @escaping @MainActor (RendererCandidateLifecycle) async throws
            -> (target: BenchmarkExplicitPaintTarget, state: State)
) async throws -> RendererCandidateMeasuredPaint<State> {
    var measuredState: State?
    let measurement = try await benchmarkMeasureExplicitCompositedPaint(
        on: screen,
        onActionStarted: { _, _ in
            try lifecycle.record(.timingStarted)
        },
        onDisplaySubmitted: {
            try lifecycle.record(.displaySubmitted)
        },
        requiredContentChangeFromObservation:
            requiredContentChangeFromObservation
    ) {
        let result = try await action(lifecycle)
        measuredState = result.state
        return result.target
    }
    guard let state = measuredState else {
        throw BenchmarkFailure.message(
            "\(lifecycle.candidate) \(lifecycle.metric.label) action "
                + "produced no candidate state"
        )
    }
    return RendererCandidateMeasuredPaint(
        state: state,
        measurement: measurement,
        lifecycleEvidence: try lifecycle.validate()
    )
}

struct RendererCandidateGeometry {
    static let contentSize = NSSize(width: 960, height: 720)
    static let styleMask: NSWindow.StyleMask = [
        .titled,
        .closable,
        .resizable,
    ]

    let screen: NSScreen
    let windowOrigin: NSPoint

    @MainActor
    static func prepare(on screen: NSScreen) throws -> Self {
        let frame = NSWindow.frameRect(
            forContentRect: NSRect(origin: .zero, size: contentSize),
            styleMask: styleMask
        )
        let available = screen.visibleFrame
        let edgeInset: CGFloat = 16
        guard frame.width + edgeInset * 2 <= available.width,
              frame.height + edgeInset * 2 <= available.height else {
            throw BenchmarkFailure.message(
                "renderer benchmark window does not fit the main display"
            )
        }
        return Self(
            screen: screen,
            windowOrigin: NSPoint(
                x: floor(available.maxX - edgeInset - frame.width),
                y: floor(available.minY + edgeInset)
            )
        )
    }

    @MainActor
    func apply(to window: NSWindow) throws {
        window.setContentSize(Self.contentSize)
        window.setFrameOrigin(windowOrigin)
        window.contentView?.layoutSubtreeIfNeeded()
        guard screen.visibleFrame.contains(window.frame) else {
            throw BenchmarkFailure.message(
                "candidate host geometry escaped the prepared display"
            )
        }
    }

    @MainActor
    func makeWindow() throws -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: Self.contentSize),
            styleMask: Self.styleMask,
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        try apply(to: window)
        return window
    }
}

@MainActor
func configureNativeBenchmarkGeometry(
    _ renderer: AppKitRenderer,
    geometry: RendererCandidateGeometry
) throws {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1 else {
        throw BenchmarkFailure.message(
            "representative native fixture must mount exactly one surface window"
        )
    }
    for window in windows {
        try geometry.apply(to: window)
    }
}
@MainActor
func explicitNativePaintTarget(
    _ renderer: AppKitRenderer
) throws -> BenchmarkExplicitPaintTarget {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1,
          let window = windows.first,
          let contentView = window.contentView,
          window.isVisible == false else {
        throw BenchmarkFailure.message(
            "native explicit paint must produce one hidden drawable surface"
        )
    }
    return BenchmarkExplicitPaintTarget(
        window: window,
        targetView: contentView
    )
}

@MainActor
func benchmarkMainScreen() throws -> NSScreen {
    guard let screen = NSScreen.main else {
        throw BenchmarkFailure.message(
            "full renderer benchmark requires a main display"
        )
    }
    return screen
}

@MainActor
func observeNativePresentation(
    _ renderer: AppKitRenderer,
    fullPaint: Bool,
    onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)? = nil
) async throws -> OnScreenPaintObservation {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1, let window = windows.first else {
        throw BenchmarkFailure.message(
            "representative native fixture must mount exactly one presentation window"
        )
    }
    guard let contentView = window.contentView else {
        throw BenchmarkFailure.message("native renderer surface has no drawable content view")
    }
    let probe = BenchmarkDrawCompletionProbe(frame: contentView.bounds)
    probe.autoresizingMask = [.width, .height]
    contentView.addSubview(probe, positioned: .above, relativeTo: nil)
    defer {
        probe.removeFromSuperview()
    }

    if fullPaint {
        return try await benchmarkObserveOnScreenPaint(
            window,
            contentDrawProbe: probe,
            onPresented: onPresented
        )
    }

    let drawCount = probe.drawCount
    let rasterized = rasterizeRenderer(renderer, showWindows: false)
    let presentedAt = clock.now
    let presentedUnixNanoseconds = benchmarkWallClockNanoseconds()
    onPresented?(presentedAt, presentedUnixNanoseconds)
    return OnScreenPaintObservation(
        crossedDisplayRefresh: rasterized && probe.drawCount > drawCount,
        captureAuthorization: false,
        pixelCaptureVerified: false,
        presentedAt: presentedAt,
        visibilityProvenance: "offscreen_appkit_raster_smoke_only",
        compositedContentEvidence: nil
    )
}

@MainActor
func applyNativeFirstState(
    plan: ProgressiveTransactionPlan,
    renderer: AppKitRenderer,
    geometry: RendererCandidateGeometry,
    lifecycle: RendererCandidateLifecycle? = nil
) throws -> SemanticStore {
    let firstWire = try SRUITransaction(
        serializedBytes: plan.firstWireBytes
    )
    let firstTransaction = try ProtocolDecoder()
        .validateAndConvertTransaction(wire: firstWire)
    var store = SemanticStore()
    guard case .success = store.applyTransactionRecord(firstTransaction) else {
        throw BenchmarkFailure.message(
            "first native transaction failed semantic application"
        )
    }
    try renderer.attach(store: store)
    try configureNativeBenchmarkGeometry(renderer, geometry: geometry)
    try lifecycle?.record(.hostAttached)
    try lifecycle?.record(.representationIngested)
    return store
}
@MainActor
func applyNativeCompleteState(
    plan: ProgressiveTransactionPlan,
    renderer: AppKitRenderer,
    geometry: RendererCandidateGeometry,
    lifecycle: RendererCandidateLifecycle? = nil
) throws -> SemanticStore {
    var store = try applyNativeFirstState(
        plan: plan,
        renderer: renderer,
        geometry: geometry
    )
    let completionWire = try SRUITransaction(
        serializedBytes: plan.completionWireBytes
    )
    let completionTransaction = try ProtocolDecoder()
        .validateAndConvertTransaction(wire: completionWire)
    guard case .success = store.applyTransactionRecord(
        completionTransaction
    ) else {
        throw BenchmarkFailure.message(
            "complete native transaction failed semantic application"
        )
    }
    try renderer.apply(
        transaction: completionTransaction,
        newStore: store
    )
    try configureNativeBenchmarkGeometry(renderer, geometry: geometry)
    try lifecycle?.record(.hostAttached)
    try lifecycle?.record(.representationIngested)
    return store
}
@MainActor
func submitNativeRendererForDisplay(
    _ renderer: AppKitRenderer,
    ordersWindow: Bool = true
) throws {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1 else {
        throw BenchmarkFailure.message(
            "native display-submission pass requires exactly one surface"
        )
    }
    if ordersWindow {
        renderer.showWindows()
    }
    for window in windows {
        if ordersWindow {
            window.orderFrontRegardless()
        }
        window.contentView?.layoutSubtreeIfNeeded()
        window.contentView?.displayIfNeeded()
        window.displayIfNeeded()
    }
    CATransaction.flush()
}
