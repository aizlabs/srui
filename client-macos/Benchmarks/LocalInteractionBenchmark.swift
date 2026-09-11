import AppKit
import Foundation
@_spi(Benchmark) import RendererAppKit
import SemanticModel
import Session

@MainActor
private final class BenchmarkHoverPointerContext {
    var screenLocation = NSPoint(x: -10_000, y: -10_000)
    var applicationIsActive = true
    var windowIsVisible = true
}

@MainActor
func benchmarkScreenCenter(
    of targetView: NSView,
    in window: NSWindow
) -> NSPoint {
    let centerInWindow = targetView.convert(
        NSPoint(x: targetView.bounds.midX, y: targetView.bounds.midY),
        to: nil
    )
    return window.convertPoint(toScreen: centerInWindow)
}

@MainActor
func benchmarkScreenPointOutside(
    _ targetView: NSView,
    in window: NSWindow
) -> NSPoint {
    let center = benchmarkScreenCenter(of: targetView, in: window)
    return NSPoint(
        x: center.x + max(targetView.bounds.width, 1) + 100,
        y: center.y
    )
}

@MainActor
func localInteractionSamples(
    renderer: AppKitRenderer,
    fixtureIndex: BenchmarkFixtureIndex,
    controller: SessionController,
    transport: BenchmarkTransport,
    sessionID: String,
    rttMilliseconds: Int,
    iterations: Int,
    fullPaint: Bool
) async throws -> LocalInteractionResult {
    let textHandle =
        renderer.registry.handle(for: fixtureIndex.textEditor)
    let textView = rendererTextView(
        renderer,
        nodeID: fixtureIndex.textEditor
    )
    let textScroll =
        (textHandle?.view as? NSScrollView)
            ?? textView?.enclosingScrollView
    let button =
        renderer.registry.view(for: fixtureIndex.primaryAction)
            as? HoverFeedbackButton
    let surface =
        renderer.registry.handle(for: fixtureIndex.surface)
    let window = surface?.window
    let host = window?.contentView
    let productionHandler = renderer.onInteraction

    guard let textView,
          let textScroll,
          let button,
          let window,
          let host,
          let productionHandler else {
        throw BenchmarkFailure.message(
            "representative native interaction controls did not mount: "
                + "text_handle=\(textHandle != nil), "
                + "text_view=\(textView != nil), "
                + "text_scroll=\(textScroll != nil), "
                + "button=\(button != nil), "
                + "surface=\(surface != nil), "
                + "window=\(window != nil), "
                + "host=\(host != nil), "
                + "production_handler=\(productionHandler != nil)"
        )
    }

    window.level = try benchmarkIsolatedHostWindowLevel()
    window.orderFrontRegardless()
    if fullPaint {
        window.makeKeyAndOrderFront(nil)
        guard window.makeFirstResponder(textView),
              window.firstResponder === textView,
              textView.inputContext != nil else {
            throw BenchmarkFailure.message(
                "full local text interaction benchmark requires the "
                    + "production NSTextView as first responder with "
                    + "an input context"
            )
        }
    }
    host.layoutSubtreeIfNeeded()
    textView.frame.size.height = max(
        textView.frame.height,
        5_000
    )

    // §31.4 measures the mounted SRUI control's local state-to-visible path.
    // OS pointer routing is deliberately outside this interval: an unbundled
    // SwiftPM executable cannot acquire foreground activation reliably on
    // supported macOS versions. The injected providers feed the production
    // reconciliation method; ScreenCaptureKit still proves the resulting
    // compositor-visible transition in full mode.
    let hoverContext = BenchmarkHoverPointerContext()
    let originalPointerProvider = button.screenPointerLocationProvider
    let originalApplicationProvider = button.applicationActiveProvider
    let originalVisibilityProvider = button.windowVisibilityProvider
    button.screenPointerLocationProvider = { hoverContext.screenLocation }
    button.applicationActiveProvider = {
        hoverContext.applicationIsActive
    }
    button.windowVisibilityProvider = { _ in hoverContext.windowIsVisible }
    defer {
        button.screenPointerLocationProvider = originalPointerProvider
        button.applicationActiveProvider = originalApplicationProvider
        button.windowVisibilityProvider = originalVisibilityProvider
        _ = button.reconcilePointerState()
    }

    hoverContext.screenLocation = benchmarkScreenPointOutside(
        button,
        in: window
    )
    guard button.reconcilePointerState() == false,
          button.isPointerInside == false else {
        throw BenchmarkFailure.message(
            "local hover benchmark could not establish its outside baseline"
        )
    }
    benchmarkSettleLocalFeedback(button)

    var callbackCount = 0
    var textEditCallbacks = [LocalTextEditCallback]()
    renderer.onInteraction = { interaction in
        callbackCount += 1
        if case .textEdit(
            let nodeID,
            let text,
            let editSeq,
            _
        ) = interaction {
            textEditCallbacks.append(
                LocalTextEditCallback(
                    nodeID: nodeID,
                    text: text,
                    editSeq: editSeq,
                    observedRevision:
                        controller.applier
                            .lastAppliedRevision.value
                )
            )
        }
        productionHandler(interaction)
    }
    defer {
        renderer.onInteraction = productionHandler
    }

    let recorder = LocalInteractionRecorder(
        window: window,
        host: host,
        progressNodeID: fixtureIndex.progress,
        controller: controller,
        transport: transport,
        rttMilliseconds: rttMilliseconds,
        fullPaint: fullPaint
    )
    var menuOpened = 0
    var hoverCompletedCompositedCycles = 0

    for _ in 0..<iterations {
        try await recorder.recordView(
            .textEntry,
            targetView: textScroll.contentView
        ) {
            let before =
                (textView.string as NSString).length
            textView.insertText(
                "x",
                replacementRange: NSRange(
                    location: before,
                    length: 0
                )
            )
            return (textView.string as NSString).length
                == before + 1
        }
    }

    for index in 0..<iterations {
        let textLength =
            (textView.string as NSString).length
        guard textLength > 0 else {
            throw BenchmarkFailure.message(
                "caret benchmark requires nonempty "
                    + "production editor text"
            )
        }
        let baselineLocation = index % textLength
        let location =
            (baselineLocation + 1) % (textLength + 1)
        textView.setSelectedRange(
            NSRange(
                location: baselineLocation,
                length: 1
            )
        )
        try await recorder.recordView(
            .caretMovement,
            targetView: textScroll.contentView
        ) {
            textView.setSelectedRange(
                NSRange(location: location, length: 0)
            )
            return textView.selectedRange().location == location
                && textView.selectedRange().length == 0
        }
    }

    for index in 0..<iterations {
        try await recorder.recordView(
            .textSelection,
            targetView: textScroll.contentView
        ) {
            let length = min(
                2,
                (textView.string as NSString).length
            )
            let availableStarts = max(
                1,
                min(
                    4,
                    (textView.string as NSString).length
                        - length + 1
                )
            )
            let location = (index + 1) % availableStarts
            let range = NSRange(
                location: location,
                length: length
            )
            textView.setSelectedRange(range)
            return textView.selectedRange() == range
        }
    }

    for _ in 0..<iterations {
        try await recorder.recordView(
            .imeComposition,
            targetView: textScroll.contentView,
            action: {
                textView.setMarkedText(
                    "é",
                    selectedRange: NSRange(
                        location: 1,
                        length: 0
                    ),
                    replacementRange: NSRange(
                        location: NSNotFound,
                        length: 0
                    )
                )
                return textView.hasMarkedText()
            },
            cleanup: {
                textView.unmarkText()
                return textView.hasMarkedText() == false
            }
        )
    }

    for index in 0..<iterations {
        try await recorder.recordView(
            .scrolling,
            targetView: textScroll.contentView
        ) {
            // Both offsets retain rendered editor content.
            let target = NSPoint(
                x: 0,
                y: index.isMultiple(of: 2) ? 23 : 0
            )
            textScroll.contentView.scroll(to: target)
            textScroll.reflectScrolledClipView(
                textScroll.contentView
            )
            return abs(
                textScroll.contentView.bounds.origin.y
                    - target.y
            ) < 1
        }
    }

    for _ in 0..<iterations {
        try await recorder.recordView(
            .hover,
            targetView: button,
            requiresExactCompositedRestoration: true,
            acceptFramesDuringAction: true,
            action: {
                hoverContext.screenLocation = benchmarkScreenCenter(
                    of: button,
                    in: window
                )
                return button.reconcilePointerState()
                    && button.isPointerInside
            },
            cleanup: {
                hoverContext.screenLocation = benchmarkScreenPointOutside(
                    button,
                    in: window
                )
                return button.reconcilePointerState() == false
                    && button.isPointerInside == false
            }
        )
        hoverCompletedCompositedCycles += 1

        hoverContext.screenLocation = benchmarkScreenCenter(
            of: button,
            in: window
        )
        guard button.reconcilePointerState(), button.isPointerInside else {
            throw BenchmarkFailure.message(
                "pressed benchmark could not establish its hovered baseline"
            )
        }
        benchmarkSettleLocalFeedback(button)

        let callbacksBefore = callbackCount
        try await recorder.recordView(
            .pressed,
            targetView: button,
            acceptFramesDuringAction: true,
            action: {
                button.performClick(nil)
                return callbackCount == callbacksBefore + 1
                    && button.isHighlighted == false
            },
            cleanup: {
                button.isHighlighted == false
                    && callbackCount == callbacksBefore + 1
            }
        )
        hoverContext.screenLocation = benchmarkScreenPointOutside(
            button,
            in: window
        )
        _ = button.reconcilePointerState()
        benchmarkSettleLocalFeedback(button)
    }

    for index in 0..<iterations {
        let menuProbe = SmokeMenuTrackingProbe()
        let visibleTextRect = textView.visibleRect
        let menuLocation = NSPoint(
            x: visibleTextRect.minX + 8,
            y: visibleTextRect.minY + 8
        )
        let contextEvent = try benchmarkMouseEvent(
            .rightMouseDown,
            window: window,
            location: textView.convert(menuLocation, to: nil),
            eventNumber: 10_000 + index
        )
        guard let productionContextMenu = textView.menu(for: contextEvent),
              productionContextMenu.items.isEmpty == false else {
            throw BenchmarkFailure.message(
                "mounted production NSTextView did not provide its native context menu"
            )
        }
        let previousDelegate = productionContextMenu.delegate
        try await recorder.record(
            .menuOpening
        ) { startInjection in
            defer {
                productionContextMenu.delegate = previousDelegate
            }
            if fullPaint {
                let measured =
                    try await benchmarkMeasureOwnedMenuPresentation(
                        productionContextMenu,
                        positioningItem: nil,
                        at: menuLocation,
                        in: textView,
                        onActionStarting: startInjection
                    )
                menuOpened += 1
                let passed =
                    menuOpened == index + 1
                        && measured.menuWindowID != 0
                        && measured.observation
                            .crossedDisplayRefresh
                        && measured.observation
                            .captureAuthorization
                        && measured.observation
                            .pixelCaptureVerified
                return LocalInteractionMeasurement(
                    latencyMilliseconds:
                        measured
                            .presentationLatencyMilliseconds,
                    passed: passed,
                    checkDetail:
                        "production_text_context_menu_surface=\(passed)"
                )
            }

            productionContextMenu.delegate = menuProbe
            try await startInjection()
            let start = clock.now
            let opensBefore = menuProbe.openCount
            let presentationsBefore =
                menuProbe.presentationCount
            menuProbe.prepare(host: host)
            let selector = #selector(
                SmokeMenuTrackingProbe
                    .observePresentationAndCancel(_:)
            )
            RunLoop.main.perform(
                selector,
                target: menuProbe,
                argument: productionContextMenu,
                order: 0,
                modes: [.eventTracking]
            )
            _ = productionContextMenu.popUp(
                positioning: nil,
                at: menuLocation,
                in: textView
            )
            menuOpened += 1
            let latency: Double
            if let observedAt = menuProbe.observedAt {
                latency = milliseconds(
                    start.duration(to: observedAt)
                )
            } else {
                latency = 0
            }
            let passed =
                menuOpened == index + 1
                    && menuProbe.openCount == opensBefore + 1
                    && menuProbe.presentationCount
                        == presentationsBefore + 1
                    && menuProbe.observedAt != nil
            return LocalInteractionMeasurement(
                latencyMilliseconds: latency,
                passed: passed,
                checkDetail:
                    "production_text_context_menu_presentation=\(passed)"
            )
        }
    }

    // Flush outside every timed local-paint interval. This separately
    // proves native editor state crossed the semantic-event and framed
    // transport boundary.
    renderer.textEditingSession.flushAllPending()
    try await waitUntil(timeout: .seconds(10)) {
        await transport.snapshot()
            .activeDelayedOperations == 0
    }

    let hoverMinimumMaterialPixelCount =
        recorder.hoverActionMaterialPixelCounts.min() ?? 0
    let hoverMaximumRestorationChannelDelta =
        recorder.hoverRestorationMaximumChannelDeltas.max()
            ?? 0

    return LocalInteractionResult(
        samples: recorder.samples,
        stateChecksPassed:
            recorder.stateChecksPassed,
        stateCheckFailures:
            recorder.stateCheckFailures,
        everyInjectedResponseUnfinishedThroughVisibleCompletion:
            recorder
                .everyInjectedResponseUnfinishedThroughVisibleCompletion,
        everyConfiguredDelayStateVerifiedAtActionStart:
            recorder
                .everyConfiguredDelayStateVerifiedAtActionStart,
        nonzeroDelayActiveAtActionStartProbeCount:
            recorder
                .nonzeroDelayActiveAtActionStartProbeCount,
        heldResponseProbeCount:
            recorder.heldResponseProbeCount,
        productionCallbacks: callbackCount,
        textEditCallbacks: textEditCallbacks,
        finalText: textView.string,
        hoverMode: fullPaint
            ? "renderer-produced NSButton changed at least "
                + "\(hoverMinimumMaterialPixelCount) target-ROI "
                + "pixels above the explicit 2/255 per-channel "
                + "SCStream tolerance after deterministic pointer-"
                + "context injection into production hover reconciliation "
                + "in \(hoverCompletedCompositedCycles)/"
                + "\(iterations) samples; the outside context then "
                + "restored every unmasked screenshot pixel "
                + "within the explicit 5/255 same-API tolerance, "
                + "with maximum observed channel delta "
                + "\(hoverMaximumRestorationChannelDelta)/255"
            : "renderer-produced NSButton completed the smoke "
                + "offscreen hover path in "
                + "\(hoverCompletedCompositedCycles)/"
                + "\(iterations) samples",
        menuMode: fullPaint
            ? "the mounted renderer NSTextView supplied its native "
                + "context menu, which was proven as a new exact "
                + "owned menu-level WindowServer surface in the "
                + "same ScreenCaptureKit frame used for its "
                + "presentation timestamp"
            : "the mounted renderer NSTextView supplied its native "
                + "context menu, which opened and was "
                + "deterministically cancelled through AppKit "
                + "event tracking in smoke mode"
    )
}
