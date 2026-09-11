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
func runSRUICandidate(
    fixture: Fixture,
    operations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool,
    driverPID: Int32
) async throws -> RendererCandidateResult {
    let plan = try progressiveTransactionPlan(
        fixture: fixture,
        operations: operations
    )
    guard let warmOperation = operations.first else {
        throw BenchmarkFailure.message(
            "representative native fixture has no warm operation"
        )
    }
    let warmStore = try makeStore([warmOperation])
    let screen = try benchmarkMainScreen()
    let geometry = try RendererCandidateGeometry.prepare(on: screen)
    let processIDs = [getpid()]
    guard let initialHostIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message(
            "native candidate process birth identity was unavailable"
        )
    }

    func makeWarmedRenderer(
        lifecycle: RendererCandidateLifecycle? = nil
    ) async throws -> AppKitRenderer {
        let renderer = AppKitRenderer()
        do {
            try renderer.attach(store: warmStore)
            try configureNativeBenchmarkGeometry(
                renderer,
                geometry: geometry
            )
            let warmObservation = try await observeNativePresentation(
                renderer,
                fullPaint: false
            )
            guard warmObservation.crossedDisplayRefresh,
                  renderer.registry.surfaceHandles.allSatisfy({
                      $0.window?.isVisible == false
                  }) else {
                throw BenchmarkFailure.message(
                    "native sample renderer did not warm offscreen"
                )
            }
            try lifecycle?.record(.warmCompleted)
            try renderer.attach(store: SemanticStore())
            guard renderer.registry.surfaceHandles.isEmpty else {
                throw BenchmarkFailure.message(
                    "native sample renderer did not reset to an empty hidden state"
                )
            }
            try lifecycle?.record(.resetCompleted)
            return renderer
        } catch {
            closeRenderer(renderer)
            throw error
        }
    }

    let startedUnixNanoseconds = benchmarkWallClockNanoseconds()
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
    var renderedPropertiesPassed = true
    var resourceAttributionComplete = true
    var contentPresentationPassed = true
    var contentPresentationPassCount = 0
    var lastContentPresentationDetail = ""
    var firstContentPresentationFailure: String?
    var lifecycleEvidence = [String]()

    for index in 0..<iterations {
        var observations = [
            RendererCandidatePaintStage: OnScreenPaintObservation
        ]()

        for metric in rendererCandidateMetricDescriptors {
            let lifecycle = fullPaint
                ? RendererCandidateLifecycle(
                    candidate: "srui",
                    metric: metric
                ) : nil
            let renderer = try await makeWarmedRenderer(
                lifecycle: lifecycle
            )
            defer {
                closeRenderer(renderer)
            }

            let store: SemanticStore
            let observation: OnScreenPaintObservation
            let latencyMilliseconds: Double
            if let lifecycle {
                let measured = try await benchmarkMeasureRendererCandidatePaint(
                    lifecycle: lifecycle,
                    on: screen,
                    requiredContentChangeFromObservation:
                        metric.includesCompletionTransaction
                            ? observations[.firstPaint] : nil
                ) { lifecycle in
                    let candidateStore =
                        metric.includesCompletionTransaction
                            ? try applyNativeCompleteState(
                                plan: plan,
                                renderer: renderer,
                                geometry: geometry,
                                lifecycle: lifecycle
                            )
                            : try applyNativeFirstState(
                                plan: plan,
                                renderer: renderer,
                                geometry: geometry,
                                lifecycle: lifecycle
                            )
                    return (
                        try explicitNativePaintTarget(renderer),
                        candidateStore
                    )
                }
                store = measured.state
                observation = measured.measurement.observation
                latencyMilliseconds =
                    measured.measurement.presentationLatencyMilliseconds
                lifecycleEvidence.append(measured.lifecycleEvidence)
            } else {
                let started = clock.now
                store = metric.includesCompletionTransaction
                    ? try applyNativeCompleteState(
                        plan: plan,
                        renderer: renderer,
                        geometry: geometry
                    )
                    : try applyNativeFirstState(
                        plan: plan,
                        renderer: renderer,
                        geometry: geometry
                    )
                observation = try await observeNativePresentation(
                    renderer,
                    fullPaint: false
                )
                latencyMilliseconds = milliseconds(
                    started.duration(to: observation.presentedAt)
                )
            }

            observations[metric.stage] = observation
            switch metric.stage {
            case .firstPaint:
                first.append(latencyMilliseconds)
                let parity = try nativeSemanticParity(
                    store: store,
                    nodes: plan.firstNodes
                )
                semanticParityPassed = semanticParityPassed && parity
                renderedPropertiesPassed = renderedPropertiesPassed
                    && nativeRenderedPropertiesMatch(
                        renderer: renderer,
                        nodes: plan.firstNodes
                    )
            case .completePaint:
                complete.append(latencyMilliseconds)
                let parity = try nativeSemanticParity(
                    store: store,
                    fixture: fixture
                )
                semanticParityPassed = semanticParityPassed && parity
                renderedPropertiesPassed = renderedPropertiesPassed
                    && nativeRenderedPropertiesMatch(
                        renderer: renderer,
                        fixture: fixture
                    )
                renderedNodeCount = renderer.registry.allHandles.count
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

        do {
            let renderer = try await makeWarmedRenderer()
            defer {
                closeRenderer(renderer)
            }
            let beforeAllocator = mallocSample()
            let beforeResources =
                benchmarkProcessResourceSample(pids: processIDs)
            let sampleStartedUnixNanoseconds =
                benchmarkWallClockNanoseconds()

            let store = try applyNativeCompleteState(
                plan: plan,
                renderer: renderer,
                geometry: geometry
            )
            try submitNativeRendererForDisplay(renderer)
            let sampleEndedUnixNanoseconds =
                benchmarkWallClockNanoseconds()
            let afterAllocator = mallocSample()
            let afterResources =
                benchmarkProcessResourceSample(pids: processIDs)

            measurementIntervals.append(
                RendererMeasurementInterval(
                    startedUnixNanoseconds:
                        sampleStartedUnixNanoseconds,
                    endedUnixNanoseconds:
                        sampleEndedUnixNanoseconds
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
            resourceAttributionComplete =
                resourceAttributionComplete
                    && beforeResources.measuredPIDCount == processIDs.count
                    && afterResources.measuredPIDCount == processIDs.count
            let parity = try nativeSemanticParity(
                store: store,
                fixture: fixture
            )
            semanticParityPassed = semanticParityPassed && parity
            renderedPropertiesPassed = renderedPropertiesPassed
                && nativeRenderedPropertiesMatch(
                    renderer: renderer,
                    fixture: fixture
                )
        }

        do {
            let renderer = try await makeWarmedRenderer()
            defer {
                closeRenderer(renderer)
            }
            let footprintSampler = ProcessFootprintSampler(
                processIDs: processIDs
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

            let store = try applyNativeCompleteState(
                plan: plan,
                renderer: renderer,
                geometry: geometry
            )
            try submitNativeRendererForDisplay(renderer)
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
            resourceAttributionComplete =
                resourceAttributionComplete
                    && footprintMeasurement.sampleCount >= 2
                    && footprintMeasurement.allTargetProcessesMeasured
            let parity = try nativeSemanticParity(
                store: store,
                fixture: fixture
            )
            semanticParityPassed = semanticParityPassed && parity
            renderedPropertiesPassed = renderedPropertiesPassed
                && nativeRenderedPropertiesMatch(
                    renderer: renderer,
                    fixture: fixture
                )
        }
    }

    let endedUnixNanoseconds = benchmarkWallClockNanoseconds()
    guard let finalHostIdentity = benchmarkProcessIdentity(pid: getpid()),
          finalHostIdentity.birthUnixNanoseconds
            == initialHostIdentity.birthUnixNanoseconds,
          finalHostIdentity.observedAliveThroughUnixNanoseconds
            >= endedUnixNanoseconds else {
        throw BenchmarkFailure.message(
            "native candidate host identity was not alive through all passes"
        )
    }
    let attribution = RendererProcessAttribution(
        candidate: "srui",
        driverPID: driverPID,
        hostPID: getpid(),
        helperPIDs: [],
        processIdentities: [finalHostIdentity],
        startedUnixNanoseconds: startedUnixNanoseconds,
        endedUnixNanoseconds: endedUnixNanoseconds,
        measurementIntervals: measurementIntervals,
        helperPIDSource:
            "native candidate has no renderer helper processes"
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
        candidate: "srui",
        expectedSampleCount: iterations,
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        hostNetLiveAllocationBlockDelta:
            hostNetLiveAllocationBlockDeltas,
        hostNetLiveAllocationByteDelta:
            hostNetLiveAllocationByteDeltas,
        hostAllocationMeasurementScope:
            sruiHostAllocationMeasurementScope,
        allocatedFootprintGrowthMiB: p50(growth),
        processFootprintPeak: peaks,
        renderedNodeCount: renderedNodeCount,
        presentationCompletions: presentationCompletions,
        representationBytes: plan.canonicalBytes.count,
        semanticParityPassed: semanticParityPassed,
        elementKindsPassed: renderedPropertiesPassed,
        resourceAttributionComplete: resourceAttributionComplete,
        captureAuthorization: captureAuthorization,
        pixelCaptureCompletions: pixelCaptureCompletions,
        contentPresentationPassed: contentPresentationPassed,
        contentPresentationDetail: contentPresentationDetail,
        paintCompletionMode: fullPaint
            ? "four disjoint passes per sample: the shared candidate lifecycle "
                + "warms and resets before timing; host attachment, prebuilt "
                + "protobuf ingestion, and display submission occur after the "
                + "common timestamp; the accepted ScreenCaptureKit frame "
                + "displayTime ends each visual interval; CPU/host-net-live-"
                + "allocation/footprint-growth and peak use separate passes"
            : "non-compositor first-state and complete-sequence timings end "
                + "at separate offscreen AppKit raster completions; no "
                + "WindowServer or compositor latency claim is made",
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
            && renderedPropertiesPassed
            && resourceAttributionComplete
            && contentPresentationPassed
            && (!fullPaint || (
                captureAuthorization
                    && pixelCaptureCompletions == requiredPixelCaptures
                    && lifecycleEvidence.count == iterations * 2
            ))
    )
}
