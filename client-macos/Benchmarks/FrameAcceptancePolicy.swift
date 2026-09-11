import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

enum FrameRejection: Error, Sendable, CustomStringConvertible {
    case beforeDisplayCutoff
    case callbackBeforeActionCompletion
    case displayGeometryChanged
    case baselineNormalizationFailed
    case candidateNormalizationFailed
    case normalizationChanged
    case candidateBlank
    case candidateUniform
    case materialComparisonFailed
    case insufficientMaterialChange(
        actual: Int,
        required: Int,
        maximumChannelDelta: Int,
        tolerance: Int
    )
    case requiredPriorNormalizationChanged
    case requiredPriorComparisonFailed
    case insufficientRequiredPriorChange(actual: Int, required: Int)
    case targetHidden
    case windowEvidenceUnavailable
    case unexpectedLayer(actual: Int, required: Int)
    case unexpectedDisplay
    case clientGeometryUnavailable
    case targetGeometryUnavailable
    case windowEvidenceChanged
    case clientGeometryChanged
    case targetGeometryChanged
    case intersectedWindow(String)
    case appKitOccluded
    case captureBoundaryRejected

    var description: String {
        switch self {
        case .beforeDisplayCutoff:
            return "frame displayTime was not after the action cutoff"
        case .callbackBeforeActionCompletion:
            return "frame callback receipt preceded action completion"
        case .displayGeometryChanged:
            return "capture display pixel geometry changed"
        case .baselineNormalizationFailed:
            return "baseline ROI normalization failed"
        case .candidateNormalizationFailed:
            return "candidate ROI normalization failed"
        case .normalizationChanged:
            return "baseline and candidate normalization differed"
        case .candidateBlank:
            return "candidate target ROI was blank"
        case .candidateUniform:
            return "candidate target ROI was uniform"
        case .materialComparisonFailed:
            return "candidate-to-baseline material pixel comparison failed"
        case let .insufficientMaterialChange(
            actual,
            required,
            maximumChannelDelta,
            tolerance
        ):
            return "candidate target ROI had \(actual) material pixel(s); "
                + "required \(required), max channel delta "
                + "\(maximumChannelDelta)/255 at \(tolerance)/255 tolerance"
        case .requiredPriorNormalizationChanged:
            return "required prior visible content used different normalization"
        case .requiredPriorComparisonFailed:
            return "required prior visible content comparison failed"
        case let .insufficientRequiredPriorChange(actual, required):
            return "candidate target ROI had \(actual) material pixel(s) "
                + "versus required prior visible content; required \(required)"
        case .targetHidden:
            return "target window was not AppKit-visible"
        case .windowEvidenceUnavailable:
            return "exact WindowServer target identity was unavailable"
        case let .unexpectedLayer(actual, required):
            return "target WindowServer layer \(actual) did not equal \(required)"
        case .unexpectedDisplay:
            return "target appeared on a different display"
        case .clientGeometryUnavailable:
            return "exact client-content geometry was unavailable"
        case .targetGeometryUnavailable:
            return "exact target-view ROI was unavailable"
        case .windowEvidenceChanged:
            return "WindowServer identity or geometry changed"
        case .clientGeometryChanged:
            return "client-content geometry changed"
        case .targetGeometryChanged:
            return "target-view ROI changed"
        case let .intersectedWindow(detail):
            return "another nonzero-alpha window intersected the target: \(detail)"
        case .appKitOccluded:
            return "target was AppKit-occluded"
        case .captureBoundaryRejected:
            return "capture boundary rejected the proven frame"
        }
    }
}

struct FrameAcceptedPixels {
    let capture: BenchmarkCompositedCaptureEvidence
    let delta: BenchmarkCompositedDeltaEvidence
}

struct FrameTargetEvidence {
    let window: BenchmarkWindowServerEvidence
    let clientContentBounds: CGRect
    let targetBounds: CGRect
}

struct FrameAcceptancePolicy {
    let channelTolerance: Int

    init(channelTolerance: Int = benchmarkStreamCaptureChannelTolerance) {
        self.channelTolerance = channelTolerance
    }

    func acceptPixels(
        frame: BenchmarkScreenCaptureFrame,
        afterDisplayTime: UInt64,
        afterReceivedAt: ContinuousClock.Instant,
        baselineFrame: BenchmarkScreenCaptureFrame,
        baselineImage: CGImage,
        candidateImage: CGImage,
        normalization suppliedNormalization:
            BenchmarkCompositedContentNormalization? = nil,
        requiredPriorContent: BenchmarkCompositedContentEvidence? = nil,
        requiredPriorNormalization:
            BenchmarkCompositedContentNormalization? = nil
    ) -> Result<FrameAcceptedPixels, FrameRejection> {
        guard frame.displayTime > afterDisplayTime else {
            return .failure(.beforeDisplayCutoff)
        }
        guard frame.receivedAt >= afterReceivedAt else {
            return .failure(.callbackBeforeActionCompletion)
        }
        guard frame.image.width == baselineFrame.image.width,
              frame.image.height == baselineFrame.image.height,
              candidateImage.width == baselineImage.width,
              candidateImage.height == baselineImage.height else {
            return .failure(.displayGeometryChanged)
        }

        let normalization = suppliedNormalization
            ?? benchmarkZeroMaskNormalization(for: baselineImage)
        guard let baselineCapture = normalizedCompositedContentEvidence(
            baselineImage,
            normalization: normalization
        ) else {
            return .failure(.baselineNormalizationFailed)
        }
        guard let candidateCapture = normalizedCompositedContentEvidence(
            candidateImage,
            normalization: normalization
        ) else {
            return .failure(.candidateNormalizationFailed)
        }
        guard candidateCapture.normalization == baselineCapture.normalization else {
            return .failure(.normalizationChanged)
        }
        guard candidateCapture.content.hasNonblankContent else {
            return .failure(.candidateBlank)
        }
        guard candidateCapture.content.hasNonuniformContent else {
            return .failure(.candidateUniform)
        }
        guard let delta = benchmarkCompositedDeltaEvidence(
            candidateCapture.content,
            baselineCapture.content,
            normalization: candidateCapture.normalization,
            channelTolerance: channelTolerance
        ) else {
            return .failure(.materialComparisonFailed)
        }
        guard delta.materiallyDifferentPixelCount
                >= delta.requiredMaterialPixelCount else {
            return .failure(
                .insufficientMaterialChange(
                    actual: delta.materiallyDifferentPixelCount,
                    required: delta.requiredMaterialPixelCount,
                    maximumChannelDelta: delta.maximumChannelDelta,
                    tolerance: delta.channelTolerance
                )
            )
        }

        switch (requiredPriorContent, requiredPriorNormalization) {
        case (nil, nil):
            break
        case let (.some(content), .some(priorNormalization)):
            guard priorNormalization == candidateCapture.normalization else {
                return .failure(.requiredPriorNormalizationChanged)
            }
            guard let priorDelta = benchmarkCompositedDeltaEvidence(
                candidateCapture.content,
                content,
                normalization: candidateCapture.normalization,
                channelTolerance: channelTolerance
            ) else {
                return .failure(.requiredPriorComparisonFailed)
            }
            guard priorDelta.materiallyDifferentPixelCount
                    >= priorDelta.requiredMaterialPixelCount else {
                return .failure(
                    .insufficientRequiredPriorChange(
                        actual: priorDelta.materiallyDifferentPixelCount,
                        required: priorDelta.requiredMaterialPixelCount
                    )
                )
            }
        default:
            return .failure(.requiredPriorComparisonFailed)
        }
        return .success(
            FrameAcceptedPixels(capture: candidateCapture, delta: delta)
        )
    }
    func acceptWindowEvidence(
        _ current: BenchmarkWindowServerEvidence?,
        requiredLayer: Int? = nil,
        requiredDisplayID: CGDirectDisplayID? = nil,
        expected: BenchmarkWindowServerEvidence? = nil,
        intersectingWindow: String?
    ) -> Result<BenchmarkWindowServerEvidence, FrameRejection> {
        guard let current else {
            return .failure(.windowEvidenceUnavailable)
        }
        if let requiredLayer, current.layer != requiredLayer {
            return .failure(
                .unexpectedLayer(actual: current.layer, required: requiredLayer)
            )
        }
        if let requiredDisplayID, current.displayID != requiredDisplayID {
            return .failure(.unexpectedDisplay)
        }
        if let expected, current != expected {
            return .failure(.windowEvidenceChanged)
        }
        if let intersectingWindow {
            return .failure(.intersectedWindow(intersectingWindow))
        }
        return .success(current)
    }

    @MainActor
    func targetEvidence(
        window: NSWindow,
        targetView: NSView,
        screen: NSScreen,
        requiredLayer: Int? = nil,
        requiredDisplayID: CGDirectDisplayID? = nil,
        expected: FrameTargetEvidence? = nil,
        requireActiveAppKitVisibility: Bool? = nil
    ) -> Result<FrameTargetEvidence, FrameRejection> {
        guard window.isVisible else {
            return .failure(.targetHidden)
        }
        guard let evidence = exactWindowServerEvidence(
            for: window,
            on: screen
        ) else {
            return .failure(.windowEvidenceUnavailable)
        }
        switch acceptWindowEvidence(
            evidence,
            requiredLayer: requiredLayer,
            requiredDisplayID: requiredDisplayID,
            expected: expected?.window,
            intersectingWindow:
                windowServerNonzeroAlphaIntersectionAbove(evidence)
        ) {
        case .success:
            break
        case let .failure(rejection):
            return .failure(rejection)
        }
        guard let clientContentBounds = exactClientContentBounds(
            for: window,
            on: screen,
            evidence: evidence
        ) else {
            return .failure(.clientGeometryUnavailable)
        }
        guard let targetBounds = exactTargetViewBounds(
            targetView,
            in: window,
            on: screen,
            windowEvidence: evidence,
            clientContentBounds: clientContentBounds
        ) else {
            return .failure(.targetGeometryUnavailable)
        }
        if let requireActiveAppKitVisibility {
            let visible = requireActiveAppKitVisibility
                ? window.occlusionState.contains(.visible)
                : window.occlusionState.rawValue != 0
            guard visible else {
                return .failure(.appKitOccluded)
            }
        }
        if let expected {
            guard clientContentBounds == expected.clientContentBounds else {
                return .failure(.clientGeometryChanged)
            }
            guard targetBounds == expected.targetBounds else {
                return .failure(.targetGeometryChanged)
            }
        }
        return .success(
            FrameTargetEvidence(
                window: evidence,
                clientContentBounds: clientContentBounds,
                targetBounds: targetBounds
            )
        )
    }

    func stableWindowEvidence(
        _ current: BenchmarkWindowServerEvidence?,
        expected: BenchmarkWindowServerEvidence
    ) -> Result<BenchmarkWindowServerEvidence, FrameRejection> {
        acceptWindowEvidence(
            current,
            expected: expected,
            intersectingWindow: current.flatMap(
                windowServerNonzeroAlphaIntersectionAbove
            )
        )
    }
}

func benchmarkAwaitChangedTargetFrame(
    from frames: AsyncStream<BenchmarkScreenCaptureFrame>,
    afterDisplayTime: UInt64,
    afterReceivedAt: ContinuousClock.Instant,
    baselineFrame: BenchmarkScreenCaptureFrame,
    baselineNormalization: BenchmarkCompositedContentNormalization
) async throws -> (
    frame: BenchmarkScreenCaptureFrame,
    capture: BenchmarkCompositedCaptureEvidence,
    delta: BenchmarkCompositedDeltaEvidence
) {
    let policy = FrameAcceptancePolicy()
    return try await withBenchmarkDeadline(
        "post-action target-ROI composited change"
    ) {
        var lastRejection: FrameRejection?
        for await frame in frames {
            switch policy.acceptPixels(
                frame: frame,
                afterDisplayTime: afterDisplayTime,
                afterReceivedAt: afterReceivedAt,
                baselineFrame: baselineFrame,
                baselineImage: baselineFrame.image,
                candidateImage: frame.image,
                normalization: baselineNormalization
            ) {
            case let .success(accepted):
                return (frame, accepted.capture, accepted.delta)
            case let .failure(rejection):
                lastRejection = rejection
            }
        }
        throw BenchmarkFailure.message(
            "ScreenCaptureKit stream ended before an acceptable target ROI "
                + "frame; last rejection: "
                + (lastRejection?.description ?? "no frame received")
        )
    }
}
