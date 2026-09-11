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

@MainActor
func loadWebDOMState(
    _ webView: WKWebView,
    probe: NavigationProbe,
    html: Data,
    expectedParity: [ParityNode],
    phase: String,
    requireAnimationFrame: Bool
) async throws {
    probe.reset()
    webView.load(
        html,
        mimeType: "text/html",
        characterEncodingName: "utf-8",
        baseURL: URL(fileURLWithPath: "/", isDirectory: true)
    )
    try await waitForWebContent(
        expectedNodeID: expectedParity.last?.id,
        requireAnimationFrame: false,
        in: webView
    )
    guard pumpWebView(
        webView,
        until: { probe.finished != nil || probe.failure != nil },
        timeout: 10
    ), probe.failure == nil else {
        throw probe.failure ?? BenchmarkFailure.message(
            "WKWebView \(phase) navigation timed out"
        )
    }
    if requireAnimationFrame {
        try await nextAnimationFrame(in: webView)
    }
}

struct WebDOMLoadRequest {
    let html: Data
    let expectedParity: [ParityNode]
    let phase: String
}

struct WebPaintMeasurement {
    let presentationLatencyMilliseconds: Double
    let observation: OnScreenPaintObservation
    let lifecycleEvidence: String?
}

@MainActor
func hideBenchmarkWindow(_ window: NSWindow) throws {
    window.orderOut(nil)
    CATransaction.flush()
    guard window.isVisible == false else {
        throw BenchmarkFailure.message(
            "benchmark window did not return to its hidden reset state"
        )
    }
}

@MainActor
func submitWebViewForDisplay(
    _ webView: WKWebView,
    window: NSWindow,
    ordersWindow: Bool = true
) throws {
    guard window.isVisible == false,
          webView.window === window else {
        throw BenchmarkFailure.message(
            "WebKit display-submission pass requires a hidden attached view"
        )
    }
    if ordersWindow {
        NSApplication.shared.activate()
        window.animationBehavior = .none
        window.makeKeyAndOrderFront(nil)
        window.orderFrontRegardless()
    }
    webView.layoutSubtreeIfNeeded()
    webView.displayIfNeeded()
    window.displayIfNeeded()
    CATransaction.flush()
}

@MainActor
func withHiddenWebHost<Result>(
    webView: WKWebView,
    geometry: RendererCandidateGeometry,
    operation: @MainActor (NSWindow) async throws -> Result
) async throws -> Result {
    guard webView.window == nil else {
        throw BenchmarkFailure.message(
            "WebKit candidate was still attached before hidden host setup"
        )
    }
    let window = try geometry.makeWindow()
    window.contentView = webView
    defer {
        if window.isVisible {
            window.orderOut(nil)
        }
        window.contentView = nil
        window.close()
    }
    return try await operation(window)
}

@MainActor
func prepareWarmedWebRenderer(
    webView: WKWebView,
    probe: NavigationProbe,
    geometry: RendererCandidateGeometry,
    warmRepresentation: Data,
    warmParity: [ParityNode],
    resetRepresentation: Data,
    lifecycle: RendererCandidateLifecycle?
) async throws {
    try await withHiddenWebHost(
        webView: webView,
        geometry: geometry
    ) { window in
        guard window.isVisible == false else {
            throw BenchmarkFailure.message(
                "WebKit warm host was visible before warm-up"
            )
        }
        try await loadWebDOMState(
            webView,
            probe: probe,
            html: warmRepresentation,
            expectedParity: warmParity,
            phase: "offscreen warm",
            requireAnimationFrame: false
        )
        let warmRasterized = try await snapshotRenderedPixels(webView)
        let warmInspection = try await inspectDOM(in: webView)
        guard warmRasterized,
              window.isVisible == false,
              warmInspection.nodes == warmParity,
              warmInspection.elementKindsPassed,
              warmInspection.renderedPropertiesPassed else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not warm offscreen"
            )
        }
        try lifecycle?.record(.warmCompleted)

        try await loadWebDOMState(
            webView,
            probe: probe,
            html: resetRepresentation,
            expectedParity: [],
            phase: "hidden reset",
            requireAnimationFrame: false
        )
        guard window.isVisible == false,
              (try await inspectDOM(in: webView)).nodes.isEmpty else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not reset while hidden"
            )
        }
        try lifecycle?.record(.resetCompleted)
    }
}

@MainActor
func resetWebRenderer(
    webView: WKWebView,
    probe: NavigationProbe,
    geometry: RendererCandidateGeometry,
    resetRepresentation: Data,
    phase: String
) async throws {
    try await withHiddenWebHost(
        webView: webView,
        geometry: geometry
    ) { window in
        try await loadWebDOMState(
            webView,
            probe: probe,
            html: resetRepresentation,
            expectedParity: [],
            phase: phase,
            requireAnimationFrame: false
        )
        guard window.isVisible == false,
              (try await inspectDOM(in: webView)).nodes.isEmpty else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not reset before \(phase)"
            )
        }
    }
}

@MainActor
func loadAndObserveWebStates(
    _ webView: WKWebView,
    probe: NavigationProbe,
    states: [WebDOMLoadRequest],
    geometry: RendererCandidateGeometry,
    fullPaint: Bool,
    lifecycle: RendererCandidateLifecycle?,
    requiredContentChangeFromObservation:
        OnScreenPaintObservation? = nil
) async throws -> WebPaintMeasurement {
    guard states.isEmpty == false else {
        throw BenchmarkFailure.message(
            "WebKit paint measurement requires at least one DOM state"
        )
    }
    if fullPaint {
        guard let lifecycle else {
            throw BenchmarkFailure.message(
                "full WebKit paint omitted shared candidate lifecycle evidence"
            )
        }
        var measuredWindow: NSWindow?
        defer {
            if let measuredWindow {
                if measuredWindow.isVisible {
                    measuredWindow.orderOut(nil)
                }
                measuredWindow.contentView = nil
                measuredWindow.close()
            }
        }
        let measured = try await benchmarkMeasureRendererCandidatePaint(
            lifecycle: lifecycle,
            on: geometry.screen,
            requiredContentChangeFromObservation:
                requiredContentChangeFromObservation
        ) { lifecycle in
            guard webView.window == nil else {
                throw BenchmarkFailure.message(
                    "WebKit measured view was attached before the timestamp"
                )
            }
            let window = try geometry.makeWindow()
            measuredWindow = window
            window.contentView = webView
            try lifecycle.record(.hostAttached)
            for state in states {
                try await loadWebDOMState(
                    webView,
                    probe: probe,
                    html: state.html,
                    expectedParity: state.expectedParity,
                    phase: state.phase,
                    requireAnimationFrame: false
                )
            }
            try lifecycle.record(.representationIngested)
            webView.layoutSubtreeIfNeeded()
            return (
                BenchmarkExplicitPaintTarget(
                    window: window,
                    targetView: webView
                ),
                window
            )
        }
        guard let measuredWindow,
              measured.state === measuredWindow else {
            throw BenchmarkFailure.message(
                "WebKit measured host lifecycle lost its window identity"
            )
        }
        return WebPaintMeasurement(
            presentationLatencyMilliseconds:
                measured.measurement.presentationLatencyMilliseconds,
            observation: measured.measurement.observation,
            lifecycleEvidence: measured.lifecycleEvidence
        )
    }

    let started = clock.now
    return try await withHiddenWebHost(
        webView: webView,
        geometry: geometry
    ) { _ in
        for state in states {
            try await loadWebDOMState(
                webView,
                probe: probe,
                html: state.html,
                expectedParity: state.expectedParity,
                phase: state.phase,
                requireAnimationFrame: false
            )
        }
        let rendered = try await snapshotRenderedPixels(webView)
        let presentedAt = clock.now
        return WebPaintMeasurement(
            presentationLatencyMilliseconds:
                milliseconds(started.duration(to: presentedAt)),
            observation: OnScreenPaintObservation(
                crossedDisplayRefresh: rendered,
                captureAuthorization: false,
                pixelCaptureVerified: false,
                presentedAt: presentedAt,
                visibilityProvenance:
                    "offscreen_wk_snapshot_smoke_only",
                compositedContentEvidence: nil
            ),
            lifecycleEvidence: nil
        )
    }
}

@MainActor
func runWebCandidate(
    fixture: Fixture,
    iterations: Int,
    fullPaint: Bool,
    driverPID: Int32
) async throws -> RendererCandidateResult {
    let firstNodeCount = try validatedFirstPaintNodeCount(fixture)
    let firstNodes = Array(fixture.nodes.prefix(firstNodeCount))
    let firstRepresentation = Data(try html(for: firstNodes).utf8)
    let completeRepresentation = Data(try html(for: fixture).utf8)
    let firstParity = fixtureParityNodes(firstNodes)
    let completeParity = fixtureParityNodes(fixture)
    let warmRepresentation = Data(
        (
            "<!doctype html><html><body><main data-srui-id=\"0\" "
                + "data-srui-type=\"Surface\" data-srui-properties=\"e30=\" "
                + "role=\"application\"></main></body></html>"
        ).utf8
    )
    let warmParity = [
        ParityNode(
            id: 0,
            type: "Surface",
            parent: nil,
            properties: [:]
        )
    ]
    let resetRepresentation = Data(
        "<!doctype html><html><body></body></html>".utf8
    )
    let screen = try benchmarkMainScreen()
    let geometry = try RendererCandidateGeometry.prepare(on: screen)
    guard let hostIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message(
            "WebKit candidate process birth identity was unavailable"
        )
    }

    let firstVisualStates = [
        WebDOMLoadRequest(
            html: firstRepresentation,
            expectedParity: firstParity,
            phase: "first useful subtree"
        )
    ]
    let completeVisualStates = [
        WebDOMLoadRequest(
            html: firstRepresentation,
            expectedParity: firstParity,
            phase: "complete-pass first DOM state"
        ),
        WebDOMLoadRequest(
            html: completeRepresentation,
            expectedParity: completeParity,
            phase: "complete representative state"
        ),
    ]

    let startedUnixNanoseconds = benchmarkWallClockNanoseconds()
    var helperPIDs = Set<pid_t>()
    var processIdentities = [pid_t: RendererProcessIdentity](
        uniqueKeysWithValues: [(getpid(), hostIdentity)]
    )
    var first = [Double]()
    var complete = [Double]()
    var cpu = [Double]()
    var hostNetLiveAllocationBlockDeltas = [Double]()
    var hostNetLiveAllocationByteDeltas = [Double]()
    var growth = [Double]()
    var peaks = [Double]()
    var measurementIntervals = [RendererMeasurementInterval]()
    var renderedNodeCount = 0
    var presentationCompletions = 0
    var pixelCaptureCompletions = 0
    var captureAuthorization = fullPaint
    var semanticParityPassed = true
    var elementKindsPassed = true
    var resourceAttributionComplete = true
    var contentPresentationPassed = true
    var contentPresentationPassCount = 0
    var lastContentPresentationDetail = ""
    var firstContentPresentationFailure: String?
    var lifecycleEvidence = [String]()

    for index in 0..<iterations {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let webView = WKWebView(
            frame: NSRect(origin: .zero, size: RendererCandidateGeometry.contentSize),
            configuration: configuration
        )
        let probe = NavigationProbe()
        webView.navigationDelegate = probe
        defer {
            webView.stopLoading()
            webView.navigationDelegate = nil
            webView.removeFromSuperview()
            pumpRunLoop(for: 0.05)
        }

        var observations = [
            RendererCandidatePaintStage: OnScreenPaintObservation
        ]()
        for metric in rendererCandidateMetricDescriptors {
            let lifecycle = fullPaint
                ? RendererCandidateLifecycle(
                    candidate: "webkit",
                    metric: metric
                ) : nil
            try await prepareWarmedWebRenderer(
                webView: webView,
                probe: probe,
                geometry: geometry,
                warmRepresentation: warmRepresentation,
                warmParity: warmParity,
                resetRepresentation: resetRepresentation,
                lifecycle: lifecycle
            )
            let states = metric.includesCompletionTransaction
                ? completeVisualStates : firstVisualStates
            let measurement = try await loadAndObserveWebStates(
                webView,
                probe: probe,
                states: states,
                geometry: geometry,
                fullPaint: fullPaint,
                lifecycle: lifecycle,
                requiredContentChangeFromObservation:
                    metric.includesCompletionTransaction
                        ? observations[.firstPaint] : nil
            )
            let observation = measurement.observation
            observations[metric.stage] = observation
            if let evidence = measurement.lifecycleEvidence {
                lifecycleEvidence.append(evidence)
            }

            let inspection = try await inspectDOM(in: webView)
            let expectedParity = metric.includesCompletionTransaction
                ? completeParity : firstParity
            semanticParityPassed = semanticParityPassed
                && inspection.nodes == expectedParity
            elementKindsPassed = elementKindsPassed
                && inspection.elementKindsPassed
                && inspection.renderedPropertiesPassed
            switch metric.stage {
            case .firstPaint:
                first.append(measurement.presentationLatencyMilliseconds)
            case .completePaint:
                complete.append(measurement.presentationLatencyMilliseconds)
                renderedNodeCount = inspection.nodes.count
            }
            if observation.crossedDisplayRefresh {
                presentationCompletions += 1
            }
            if observation.pixelCaptureVerified {
                pixelCaptureCompletions += 1
            }
            captureAuthorization =
                captureAuthorization && observation.captureAuthorization
        }

        guard let firstObservation = observations[.firstPaint],
              let completeObservation = observations[.completePaint] else {
            throw BenchmarkFailure.message(
                "renderer metric descriptor table omitted a paint stage"
            )
        }
        let contentCheck = progressiveContentEvidenceCheck(
            first: firstObservation,
            complete: completeObservation,
            fullPaint: fullPaint
        )
        contentPresentationPassed =
            contentPresentationPassed && contentCheck.passed
        if contentCheck.passed {
            contentPresentationPassCount += 1
        } else if firstContentPresentationFailure == nil {
            firstContentPresentationFailure =
                "sample \(index): \(contentCheck.detail)"
        }
        lastContentPresentationDetail =
            "sample \(index): \(contentCheck.detail)"

        try await resetWebRenderer(
            webView: webView,
            probe: probe,
            geometry: geometry,
            resetRepresentation: resetRepresentation,
            phase: "resource pass"
        )
        let beforeHelperPIDsByRole =
            benchmarkWebKitHelperProcessIDsByRole(webView)
        let beforeHelperPIDs = Set(beforeHelperPIDsByRole.values)
            .subtracting([getpid()])
        let beforeHelperIdentities = beforeHelperPIDs.compactMap {
            benchmarkProcessIdentity(pid: $0)
        }
        guard let sampleWebContentPID =
                beforeHelperPIDsByRole["webcontent"],
              beforeHelperPIDs.contains(sampleWebContentPID) else {
            throw BenchmarkFailure.message(
                "WebKit sample has no exact pre-measurement WebContent PID"
            )
        }
        let measurementPIDs = [getpid()] + beforeHelperPIDs.sorted()
        var sampleResourcesComplete =
            beforeHelperPIDs.isEmpty == false
                && beforeHelperIdentities.count == beforeHelperPIDs.count
        for identity in beforeHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds
                    != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
        }
        helperPIDs.formUnion(beforeHelperPIDs)

        let beforeAllocator = mallocSample()
        let beforeResources = benchmarkProcessResourceSample(
            pids: measurementPIDs
        )
        sampleResourcesComplete =
            sampleResourcesComplete
                && beforeResources.measuredPIDCount == measurementPIDs.count
        let sampleStartedUnixNanoseconds =
            benchmarkWallClockNanoseconds()
        var resourceEndedUnixNanoseconds: UInt64?

        let resourceWindow = try geometry.makeWindow()
        resourceWindow.contentView = webView
        do {
            defer {
                if resourceWindow.isVisible {
                    resourceWindow.orderOut(nil)
                }
                resourceWindow.contentView = nil
                resourceWindow.close()
            }
            for state in completeVisualStates {
                try await loadWebDOMState(
                    webView,
                    probe: probe,
                    html: state.html,
                    expectedParity: state.expectedParity,
                    phase: "resource \(state.phase)",
                    requireAnimationFrame: false
                )
            }
            try submitWebViewForDisplay(
                webView,
                window: resourceWindow
            )
            let ended = benchmarkWallClockNanoseconds()
            resourceEndedUnixNanoseconds = ended
            let afterAllocator = mallocSample()
            let afterResources = benchmarkProcessResourceSample(
                pids: measurementPIDs
            )

            measurementIntervals.append(
                RendererMeasurementInterval(
                    startedUnixNanoseconds:
                        sampleStartedUnixNanoseconds,
                    endedUnixNanoseconds: ended
                )
            )
            cpu.append(
                max(
                    0,
                    afterResources.cpuMilliseconds
                        - beforeResources.cpuMilliseconds
                )
            )
            hostNetLiveAllocationBlockDeltas.append(
                Double(afterAllocator.blocks - beforeAllocator.blocks)
            )
            hostNetLiveAllocationByteDeltas.append(
                Double(afterAllocator.bytes - beforeAllocator.bytes)
            )
            growth.append(
                max(
                    0,
                    afterResources.physicalFootprintMiB
                        - beforeResources.physicalFootprintMiB
                )
            )
            sampleResourcesComplete =
                sampleResourcesComplete
                    && afterResources.measuredPIDCount
                        == measurementPIDs.count
            let inspection = try await inspectDOM(in: webView)
            semanticParityPassed = semanticParityPassed
                && inspection.nodes == completeParity
            elementKindsPassed = elementKindsPassed
                && inspection.elementKindsPassed
                && inspection.renderedPropertiesPassed
            try hideBenchmarkWindow(resourceWindow)
        } catch {
            if resourceWindow.isVisible {
                try? hideBenchmarkWindow(resourceWindow)
            }
            throw error
        }

        guard let resourceEndedUnixNanoseconds else {
            throw BenchmarkFailure.message(
                "WebKit resource pass produced no display submission"
            )
        }
        let afterResourceHelperPIDs = Set(
            benchmarkWebKitHelperProcessIDs(webView)
        ).subtracting([getpid()])
        let afterResourceHelperIdentities =
            afterResourceHelperPIDs.compactMap {
                benchmarkProcessIdentity(pid: $0)
            }
        sampleResourcesComplete =
            sampleResourcesComplete
                && afterResourceHelperPIDs == beforeHelperPIDs
                && afterResourceHelperIdentities.count
                    == afterResourceHelperPIDs.count
                && afterResourceHelperIdentities.allSatisfy {
                    $0.observedAliveThroughUnixNanoseconds
                        >= resourceEndedUnixNanoseconds
                }
        for identity in afterResourceHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds
                    != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
        }
        helperPIDs.formUnion(afterResourceHelperPIDs)

        try await resetWebRenderer(
            webView: webView,
            probe: probe,
            geometry: geometry,
            resetRepresentation: resetRepresentation,
            phase: "peak pass"
        )
        let peakHelperPIDsByRole =
            benchmarkWebKitHelperProcessIDsByRole(webView)
        let peakHelperPIDs = Set(peakHelperPIDsByRole.values)
            .subtracting([getpid()])
        guard peakHelperPIDsByRole == beforeHelperPIDsByRole,
              peakHelperPIDs == beforeHelperPIDs else {
            throw BenchmarkFailure.message(
                "WebKit exact helper PID-role topology changed between "
                    + "resource and peak passes"
            )
        }

        let footprintSampler = ProcessFootprintSampler(
            processIDs: measurementPIDs
        )
        var footprintSamplerFinished = false
        defer {
            if footprintSamplerFinished == false {
                footprintSampler.cancel()
            }
        }
        let peakStartedUnixNanoseconds =
            benchmarkWallClockNanoseconds()
        footprintSampler.begin(at: peakStartedUnixNanoseconds)

        let peakWindow = try geometry.makeWindow()
        peakWindow.contentView = webView
        defer {
            if peakWindow.isVisible {
                peakWindow.orderOut(nil)
            }
            peakWindow.contentView = nil
            peakWindow.close()
        }
        for state in completeVisualStates {
            try await loadWebDOMState(
                webView,
                probe: probe,
                html: state.html,
                expectedParity: state.expectedParity,
                phase: "peak \(state.phase)",
                requireAnimationFrame: false
            )
        }
        try submitWebViewForDisplay(
            webView,
            window: peakWindow
        )
        footprintSampler.sampleNow()
        let peakEndedUnixNanoseconds =
            benchmarkWallClockNanoseconds()
        let footprintMeasurement = footprintSampler.finish(
            at: peakEndedUnixNanoseconds
        )
        footprintSamplerFinished = true

        peaks.append(
            footprintMeasurement.peakPhysicalFootprintMiB
        )
        let peakInspection = try await inspectDOM(in: webView)
        semanticParityPassed = semanticParityPassed
            && peakInspection.nodes == completeParity
        elementKindsPassed = elementKindsPassed
            && peakInspection.elementKindsPassed
            && peakInspection.renderedPropertiesPassed
        try hideBenchmarkWindow(peakWindow)

        let afterPeakHelperPIDs = Set(
            benchmarkWebKitHelperProcessIDs(webView)
        ).subtracting([getpid()])
        let afterPeakHelperIdentities = afterPeakHelperPIDs.compactMap {
            benchmarkProcessIdentity(pid: $0)
        }
        sampleResourcesComplete =
            sampleResourcesComplete
                && footprintMeasurement.sampleCount >= 2
                && footprintMeasurement.allTargetProcessesMeasured
                && afterPeakHelperPIDs == beforeHelperPIDs
                && afterPeakHelperIdentities.count
                    == afterPeakHelperPIDs.count
                && afterPeakHelperIdentities.allSatisfy {
                    $0.observedAliveThroughUnixNanoseconds
                        >= peakEndedUnixNanoseconds
                }
        for identity in afterPeakHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds
                    != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
        }
        helperPIDs.formUnion(afterPeakHelperPIDs)

        guard let afterHostIdentity =
                benchmarkProcessIdentity(pid: getpid()),
              afterHostIdentity.birthUnixNanoseconds
                == hostIdentity.birthUnixNanoseconds,
              afterHostIdentity.observedAliveThroughUnixNanoseconds
                >= peakEndedUnixNanoseconds else {
            throw BenchmarkFailure.message(
                "WebKit host identity was not alive through resource "
                    + "and peak passes"
            )
        }
        processIdentities[getpid()] = afterHostIdentity
        let webContentIdentified =
            afterPeakHelperIdentities.contains {
                $0.pid == sampleWebContentPID
            }
        resourceAttributionComplete =
            resourceAttributionComplete
                && webContentIdentified
                && sampleResourcesComplete
    }

    let endedUnixNanoseconds = benchmarkWallClockNanoseconds()
    let attributedPIDs = Set([getpid()]).union(helperPIDs)
    resourceAttributionComplete =
        resourceAttributionComplete
            && Set(processIdentities.keys) == attributedPIDs
    let attribution = RendererProcessAttribution(
        candidate: "webkit",
        driverPID: driverPID,
        hostPID: getpid(),
        helperPIDs: helperPIDs.sorted(),
        processIdentities:
            processIdentities.values.sorted { $0.pid < $1.pid },
        startedUnixNanoseconds: startedUnixNanoseconds,
        endedUnixNanoseconds: endedUnixNanoseconds,
        measurementIntervals: measurementIntervals,
        helperPIDSource:
            "required WebContent PID from benchmark-only "
                + "_webProcessIdentifier; optional "
                + "_networkProcessIdentifier/_gpuProcessIdentifier values "
                + "included when available; every exact PID-role mapping "
                + "must remain unchanged through the separate resource "
                + "and peak-footprint passes"
    )
    let contentPresentationDetail =
        "\(contentPresentationPassCount)/\(iterations) samples passed; "
            + (
                firstContentPresentationFailure
                    ?? lastContentPresentationDetail
            )
            + (lifecycleEvidence.isEmpty
                ? ""
                : "; shared candidate lifecycle: "
                    + lifecycleEvidence.joined(separator: " | "))
    let requiredPixelCaptures = fullPaint ? iterations * 2 : 0
    return RendererCandidateResult(
        candidate: "webkit",
        expectedSampleCount: iterations,
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        hostNetLiveAllocationBlockDelta:
            hostNetLiveAllocationBlockDeltas,
        hostNetLiveAllocationByteDelta:
            hostNetLiveAllocationByteDeltas,
        hostAllocationMeasurementScope:
            webKitHostAllocationMeasurementScope,
        allocatedFootprintGrowthMiB: p50(growth),
        processFootprintPeak: peaks,
        renderedNodeCount: renderedNodeCount,
        presentationCompletions: presentationCompletions,
        representationBytes:
            firstRepresentation.count + completeRepresentation.count,
        semanticParityPassed: semanticParityPassed,
        elementKindsPassed: elementKindsPassed,
        resourceAttributionComplete: resourceAttributionComplete,
        captureAuthorization: captureAuthorization,
        pixelCaptureCompletions: pixelCaptureCompletions,
        contentPresentationPassed: contentPresentationPassed,
        contentPresentationDetail: contentPresentationDetail,
        paintCompletionMode: fullPaint
            ? "four disjoint passes per sample: the shared candidate lifecycle "
                + "warms and resets before timing; measured NSWindow creation "
                + "and attachment, prebuilt HTML-byte ingestion, and display "
                + "submission occur after the common timestamp; the accepted "
                + "ScreenCaptureKit frame displayTime ends each visual interval; "
                + "CPU/host-net-live-allocation/footprint-growth and peak use "
                + "separate measured-host passes"
            : "non-compositor first-state and complete-sequence timings include "
                + "measured host creation/attachment and end at separate "
                + "offscreen WKSnapshot completions; no WindowServer or "
                + "compositor latency claim is made",
        attribution: attribution,
        succeeded: first.count == iterations
            && complete.count == iterations
            && cpu.count == iterations
            && hostNetLiveAllocationBlockDeltas.count == iterations
            && hostNetLiveAllocationByteDeltas.count == iterations
            && peaks.count == iterations
            && renderedNodeCount == fixture.nodes.count
            && presentationCompletions == iterations * 2
            && semanticParityPassed
            && elementKindsPassed
            && resourceAttributionComplete
            && contentPresentationPassed
            && (!fullPaint || (
                captureAuthorization
                    && pixelCaptureCompletions == requiredPixelCaptures
                    && lifecycleEvidence.count == iterations * 2
            ))
    )
}
