import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

struct PassiveFrameAcceptanceBoundary: Equatable {
    let displayTime: UInt64
    let receivedAt: ContinuousClock.Instant
}

func benchmarkPassiveFrameAcceptanceBoundary(
    acceptFramesDuringAction: Bool,
    actionStartedMachTicks: UInt64,
    actionStartedAt: ContinuousClock.Instant,
    actionCompletedMachTicks: UInt64,
    actionCompletedAt: ContinuousClock.Instant
) -> PassiveFrameAcceptanceBoundary {
    if acceptFramesDuringAction {
        return PassiveFrameAcceptanceBoundary(
            displayTime: actionStartedMachTicks,
            receivedAt: actionStartedAt
        )
    }
    return PassiveFrameAcceptanceBoundary(
        displayTime: actionCompletedMachTicks,
        receivedAt: actionCompletedAt
    )
}

@MainActor
func benchmarkMeasurePassiveCompositedChange(
    _ window: NSWindow,
    targetView: NSView,
    acceptFramesDuringAction: Bool = false,
    onActionStarting:
        (@MainActor () async throws -> Void)? = nil,
    restorationAction:
        (@MainActor () throws -> Bool)? = nil,
    action: @escaping @MainActor () async throws -> Void
) async throws -> BenchmarkPassivePresentationMeasurement {
    let prepared = try await benchmarkPrepareExactVisibleWindow(window)
    guard let targetBounds = exactTargetViewBounds(
        targetView,
        in: window,
        on: prepared.screen,
        windowEvidence: prepared.evidence,
        clientContentBounds: prepared.clientContentBounds
    ) else {
        throw BenchmarkFailure.message(
            "passive measurement could not freeze an exact visible target-view ROI"
        )
    }

    let policy = FrameAcceptancePolicy()
    let expectedTarget = FrameTargetEvidence(
        window: prepared.evidence,
        clientContentBounds: prepared.clientContentBounds,
        targetBounds: targetBounds
    )
    func requireStableTarget(_ phase: String) throws -> FrameTargetEvidence {
        switch policy.targetEvidence(
            window: window,
            targetView: targetView,
            screen: prepared.screen,
            requiredDisplayID: prepared.evidence.displayID,
            expected: expectedTarget
        ) {
        case let .success(evidence):
            return evidence
        case let .failure(rejection):
            throw BenchmarkFailure.message(
                "\(phase) frame rejected: \(rejection.description)"
            )
        }
    }
    _ = try requireStableTarget("pre-capture")

    let captureSession = try await benchmarkStartScreenCaptureStream(
        displayID: prepared.evidence.displayID,
        globalSourceRect: targetBounds,
        operation: "passive target-ROI capture"
    )
    do {
        let baselineFrame = try await benchmarkAwaitFirstCompleteFrame(
            from: captureSession.frames,
            operation: "passive target-ROI baseline"
        )
        guard let baselineCapture =
            benchmarkNormalizedFrameEvidence(baselineFrame),
              baselineCapture.content.hasNonblankContent,
              baselineCapture.content.hasNonuniformContent else {
            throw BenchmarkFailure.message(
                "passive target-ROI baseline was blank or uniform"
            )
        }
        let preActionTarget = try requireStableTarget("pre-action")

        let restorationReferenceCapture:
            BenchmarkCompositedCaptureEvidence?
        if restorationAction != nil {
            let reference = try await captureCompositedClientContent(
                evidence: preActionTarget.window,
                clientContentBounds: targetBounds,
                normalization: baselineCapture.normalization
            )
            guard reference.content.hasNonblankContent,
                  reference.content.hasNonuniformContent,
                  reference.normalization == baselineCapture.normalization else {
                throw BenchmarkFailure.message(
                    "pre-action restoration reference screenshot was blank, "
                        + "uniform, or used different normalization"
                )
            }
            restorationReferenceCapture = reference
        } else {
            restorationReferenceCapture = nil
        }

        benchmarkTrace(
            "passive baseline window=\(prepared.evidence.windowID) "
                + "target=\(targetBounds) mouse=\(NSEvent.mouseLocation) "
                + "fingerprint="
                + String(
                    baselineCapture.content.normalizedFingerprintSHA256
                        .prefix(16)
                )
        )

        try await onActionStarting?()
        let actionStartedMachTicks = mach_absolute_time()
        let actionStartedAt = clock.now
        try await action()

        let actionCompletedAt = clock.now
        let actionCompletedMachTicks = mach_absolute_time()
        let acceptanceBoundary = benchmarkPassiveFrameAcceptanceBoundary(
            acceptFramesDuringAction: acceptFramesDuringAction,
            actionStartedMachTicks: actionStartedMachTicks,
            actionStartedAt: actionStartedAt,
            actionCompletedMachTicks: actionCompletedMachTicks,
            actionCompletedAt: actionCompletedAt
        )
        let changed = try await benchmarkAwaitChangedTargetFrame(
            from: captureSession.frames,
            afterDisplayTime: acceptanceBoundary.displayTime,
            afterReceivedAt: acceptanceBoundary.receivedAt,
            baselineFrame: baselineFrame,
            baselineNormalization: baselineCapture.normalization
        )
        _ = try requireStableTarget("accepted post-action")

        var captureEquivalentBaselineRestorationVerified = false
        var restorationDeltaEvidence: BenchmarkCompositedDeltaEvidence?
        if let restorationAction {
            guard let restorationReferenceCapture else {
                throw BenchmarkFailure.message(
                    "composited restoration omitted its same-API reference capture"
                )
            }
            await captureSession.stop()
            let restorationStateCorrect = try restorationAction()
            targetView.displayIfNeeded()
            window.displayIfNeeded()
            CATransaction.flush()
            guard restorationStateCorrect else {
                throw BenchmarkFailure.message(
                    "target restoration action reported an invalid local state"
                )
            }
            let restored = try await benchmarkCaptureRestoredTarget(
                window: window,
                screen: prepared.screen,
                expectedWindowEvidence: prepared.evidence,
                targetBounds: targetBounds,
                baselineImage: restorationReferenceCapture.image,
                normalization: restorationReferenceCapture.normalization
            )
            _ = try requireStableTarget("restored")
            captureEquivalentBaselineRestorationVerified = true
            restorationDeltaEvidence = restored.delta
        }

        let provenance =
            "screencapturekit_same_complete_frame_target_roi_after_action_"
                + (captureEquivalentBaselineRestorationVerified
                    ? "capture_equivalent_baseline_restored_" : "")
                + prepared.visibilityProvenance
        let observation = OnScreenPaintObservation(
            crossedDisplayRefresh: true,
            captureAuthorization: true,
            pixelCaptureVerified: true,
            presentedAt: changed.frame.receivedAt,
            visibilityProvenance: provenance,
            compositedContentEvidence: changed.capture.content,
            compositedContentNormalization: changed.capture.normalization
        )
        await captureSession.stop()
        benchmarkTrace(
            "passive target paint provenance=\(provenance) "
                + "window=\(prepared.evidence.windowID) "
                + "fingerprint="
                + String(
                    changed.capture.content.normalizedFingerprintSHA256
                        .prefix(16)
                )
                + " restoration="
                + "\(captureEquivalentBaselineRestorationVerified)"
        )
        return BenchmarkPassivePresentationMeasurement(
            actionStartedAt: actionStartedAt,
            actionCompletedAt: actionCompletedAt,
            actionStartedMachTicks: actionStartedMachTicks,
            acceptedDisplayMachTicks: changed.frame.displayTime,
            presentationLatencyMilliseconds:
                try benchmarkMachElapsedMilliseconds(
                    from: actionStartedMachTicks,
                    to: changed.frame.displayTime
                ),
            observation: observation,
            contentDeltaEvidence: changed.delta,
            restorationDeltaEvidence: restorationDeltaEvidence
        )
    } catch {
        await captureSession.stop()
        throw error
    }
}
