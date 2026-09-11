import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

struct BenchmarkPassivePresentationMeasurement: Sendable {
    let actionStartedAt: ContinuousClock.Instant
    let actionCompletedAt: ContinuousClock.Instant
    let actionStartedMachTicks: UInt64
    let acceptedDisplayMachTicks: UInt64
    let presentationLatencyMilliseconds: Double
    let observation: OnScreenPaintObservation
    let contentDeltaEvidence: BenchmarkCompositedDeltaEvidence?
    let restorationDeltaEvidence: BenchmarkCompositedDeltaEvidence?

    init(
        actionStartedAt: ContinuousClock.Instant,
        actionCompletedAt: ContinuousClock.Instant,
        actionStartedMachTicks: UInt64,
        acceptedDisplayMachTicks: UInt64,
        presentationLatencyMilliseconds: Double,
        observation: OnScreenPaintObservation,
        contentDeltaEvidence: BenchmarkCompositedDeltaEvidence? = nil,
        restorationDeltaEvidence: BenchmarkCompositedDeltaEvidence? = nil
    ) {
        self.actionStartedAt = actionStartedAt
        self.actionCompletedAt = actionCompletedAt
        self.actionStartedMachTicks = actionStartedMachTicks
        self.acceptedDisplayMachTicks = acceptedDisplayMachTicks
        self.presentationLatencyMilliseconds = presentationLatencyMilliseconds
        self.observation = observation
        self.contentDeltaEvidence = contentDeltaEvidence
        self.restorationDeltaEvidence = restorationDeltaEvidence
    }
}

struct BenchmarkMenuPresentationMeasurement: Sendable {
    let actionStartedAt: ContinuousClock.Instant
    let actionStartedMachTicks: UInt64
    let acceptedDisplayMachTicks: UInt64
    let presentationLatencyMilliseconds: Double
    let observation: OnScreenPaintObservation
    let menuWindowID: CGWindowID
}

func benchmarkMachElapsedMilliseconds(
    from startedMachTicks: UInt64,
    to endedMachTicks: UInt64
) throws -> Double {
    guard endedMachTicks >= startedMachTicks else {
        throw BenchmarkFailure.message(
            "ScreenCaptureKit display time preceded the measured action"
        )
    }
    var timebase = mach_timebase_info_data_t()
    guard mach_timebase_info(&timebase) == KERN_SUCCESS,
          timebase.denom != 0 else {
        throw BenchmarkFailure.message(
            "mach_timebase_info was unavailable for presentation latency"
        )
    }
    let elapsedTicks = endedMachTicks - startedMachTicks
    let elapsedNanoseconds = Double(elapsedTicks)
        * Double(timebase.numer)
        / Double(timebase.denom)
    let elapsedMilliseconds = elapsedNanoseconds / 1_000_000
    guard elapsedMilliseconds.isFinite, elapsedMilliseconds >= 0 else {
        throw BenchmarkFailure.message(
            "mach presentation latency was non-finite"
        )
    }
    return elapsedMilliseconds
}

enum BenchmarkScreenCaptureFrameDisposition {
    case continuous
    case baseline
    case drop
    case postArmCandidate
}

final class BenchmarkScreenCaptureBoundary:
    @unchecked Sendable
{
    enum State {
        case needsBaseline
        case waitingForArm
        case armed(UInt64)
        case accepted
    }

    let lock = NSLock()
    let onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)?
    var state = State.needsBaseline

    init(
        onPresented:
            (@MainActor (ContinuousClock.Instant, UInt64) -> Void)?
    ) {
        self.onPresented = onPresented
    }

    func arm(afterDisplayTime cutoffDisplayTime: UInt64) {
        lock.withLock {
            guard case .waitingForArm = state else { return }
            state = .armed(cutoffDisplayTime)
        }
    }

    func disposition(
        for displayTime: UInt64
    ) -> BenchmarkScreenCaptureFrameDisposition {
        lock.withLock {
            switch state {
            case .needsBaseline:
                // Convert/yield exactly one baseline before instrumentation.
                state = .waitingForArm
                return .baseline
            case .waitingForArm:
                // The measured production action is running. Drop before
                // CVPixelBuffer conversion or AsyncStream buffering.
                return .drop
            case .armed(let cutoffDisplayTime):
                guard displayTime > cutoffDisplayTime else {
                    return .drop
                }
                // Do not claim the first refresh after arm. It may have raced
                // ahead of WindowServer's order transaction. Yield successive
                // post-cutoff frames until exact same-frame evidence accepts
                // one of them.
                return .postArmCandidate
            case .accepted:
                return .drop
            }
        }
    }

    @MainActor
    func accept(
        displayTime: UInt64,
        receivedAt: ContinuousClock.Instant,
        receivedUnixNanoseconds: UInt64
    ) -> Bool {
        let didAccept = lock.withLock {
            guard case .armed(let cutoffDisplayTime) = state,
                  displayTime > cutoffDisplayTime else {
                return false
            }
            state = .accepted
            return true
        }
        guard didAccept else { return false }

        // Invoke the resource callback only for the accepted candidate. Its
        // arguments remain the SCStream callback-receipt metadata recorded
        // before conversion and exact-pixel verification.
        onPresented?(receivedAt, receivedUnixNanoseconds)
        return true
    }
}

struct BenchmarkScreenCaptureFrame: @unchecked Sendable {
    let displayTime: UInt64
    let receivedAt: ContinuousClock.Instant
    let receivedUnixNanoseconds: UInt64
    let isPostArmCandidate: Bool
    let image: CGImage
}

final class BenchmarkScreenCaptureOutput:
    NSObject,
    SCStreamOutput,
    @unchecked Sendable
{
    let continuation:
        AsyncStream<BenchmarkScreenCaptureFrame>.Continuation
    let presentationBoundary: BenchmarkScreenCaptureBoundary?

    init(
        continuation:
            AsyncStream<BenchmarkScreenCaptureFrame>.Continuation,
        presentationBoundary: BenchmarkScreenCaptureBoundary?
    ) {
        self.continuation = continuation
        self.presentationBoundary = presentationBoundary
    }

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of outputType: SCStreamOutputType
    ) {
        // These are captured at callback receipt. Attachment parsing is the
        // only work before the optional resource-boundary callback.
        let receivedAt = clock.now
        guard outputType == .screen,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(
                  sampleBuffer,
                  createIfNecessary: false
              ) as? [[SCStreamFrameInfo: Any]],
              let attachment = attachments.first,
              let statusNumber =
                  attachment[SCStreamFrameInfo.status] as? NSNumber,
              SCFrameStatus(rawValue: statusNumber.intValue) == .complete,
              let displayTimeNumber =
                  attachment[SCStreamFrameInfo.displayTime] as? NSNumber,
              let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer)
        else {
            return
        }

        let displayTime = displayTimeNumber.uint64Value
        let disposition = presentationBoundary?.disposition(
            for: displayTime
        ) ?? .continuous
        guard disposition != .drop else {
            // In explicit §31.1 timing this is the only per-frame work while
            // the action runs: callback entry, one monotonic clock read, and
            // ScreenCaptureKit attachment/status parsing.
            return
        }

        let receivedUnixNanoseconds = benchmarkWallClockNanoseconds()
        let isPostArmCandidate = disposition == .postArmCandidate

        var image: CGImage?
        guard VTCreateCGImageFromCVPixelBuffer(
            pixelBuffer,
            options: nil,
            imageOut: &image
        ) == noErr,
              let image else {
            return
        }
        continuation.yield(
            BenchmarkScreenCaptureFrame(
                displayTime: displayTime,
                receivedAt: receivedAt,
                receivedUnixNanoseconds: receivedUnixNanoseconds,
                isPostArmCandidate: isPostArmCandidate,
                image: image
            )
        )
    }

    func finish() {
        continuation.finish()
    }
}

final class BenchmarkScreenCaptureSession: @unchecked Sendable {
    let frames: AsyncStream<BenchmarkScreenCaptureFrame>

    let stream: SCStream
    let output: BenchmarkScreenCaptureOutput
    let sampleQueue: DispatchQueue
    let lock = NSLock()
    var stopped = false

    init(
        frames: AsyncStream<BenchmarkScreenCaptureFrame>,
        stream: SCStream,
        output: BenchmarkScreenCaptureOutput,
        sampleQueue: DispatchQueue
    ) {
        self.frames = frames
        self.stream = stream
        self.output = output
        self.sampleQueue = sampleQueue
    }

    func finishFrames() {
        output.finish()
    }

    func stop() async {
        let shouldStop = lock.withLock {
            let result = stopped == false
            stopped = true
            return result
        }
        guard shouldStop else { return }

        try? await stream.stopCapture()
        try? stream.removeStreamOutput(output, type: .screen)
        output.finish()
        sampleQueue.sync {}
    }
}

struct BenchmarkOwnedMenuFrameProof: Sendable {
    let windowEvidence: BenchmarkWindowServerEvidence
    let contentEvidence: BenchmarkCompositedContentEvidence
    let normalization: BenchmarkCompositedContentNormalization
    let presentedAt: ContinuousClock.Instant
    let acceptedDisplayMachTicks: UInt64
}

final class BenchmarkMenuCancellationTarget:
    NSObject,
    @unchecked Sendable
{
    weak var menu: NSMenu?

    init(menu: NSMenu) {
        self.menu = menu
    }

    nonisolated func schedule() {
        RunLoop.main.perform(
            #selector(cancelMenu(_:)),
            target: self,
            argument: nil,
            order: 0,
            modes: [.eventTracking, .common]
        )
    }

    @MainActor
    @objc func cancelMenu(_ ignored: Any?) {
        menu?.cancelTrackingWithoutAnimation()
    }
}

func benchmarkZeroMaskNormalization(
    for image: CGImage
) -> BenchmarkCompositedContentNormalization {
    BenchmarkCompositedContentNormalization(
        pixelWidth: image.width,
        pixelHeight: image.height,
        markerMaskMinX: 0,
        markerMaskMaxX: 0,
        markerMaskMinY: 0,
        markerMaskMaxY: 0
    )
}

func benchmarkNormalizedFrameEvidence(
    _ frame: BenchmarkScreenCaptureFrame
) -> BenchmarkCompositedCaptureEvidence? {
    normalizedCompositedContentEvidence(
        frame.image,
        normalization: benchmarkZeroMaskNormalization(for: frame.image)
    )
}

@MainActor
func exactTargetViewBounds(
    _ targetView: NSView,
    in window: NSWindow,
    on screen: NSScreen,
    windowEvidence: BenchmarkWindowServerEvidence,
    clientContentBounds: CGRect
) -> CGRect? {
    guard targetView.window === window else { return nil }

    var ancestor: NSView? = targetView
    while let view = ancestor {
        guard view.isHidden == false, view.alphaValue > 0 else {
            return nil
        }
        ancestor = view.superview
    }

    let targetVisibleRect = targetView.visibleRect.intersection(
        targetView.bounds
    )
    guard targetVisibleRect.isNull == false,
          targetVisibleRect.width > 2,
          targetVisibleRect.height > 2 else {
        return nil
    }
    let windowRect = targetView.convert(targetVisibleRect, to: nil)
    let screenRect = window.convertToScreen(windowRect)
    let displayBounds = CGDisplayBounds(windowEvidence.displayID)
    var bounds = CGRect(
        x: displayBounds.minX + screenRect.minX - screen.frame.minX,
        y: displayBounds.minY + screen.frame.maxY - screenRect.maxY,
        width: screenRect.width,
        height: screenRect.height
    )
    // Exclude anti-aliased clipping edges and neighbouring controls. The
    // frozen ROI must remain wholly inside both the exact client area and the
    // supplied target view for the complete measurement.
    bounds = bounds.insetBy(dx: 1, dy: 1)
    guard bounds.width > 0,
          bounds.height > 0,
          clientContentBounds.contains(bounds),
          displayBounds.contains(bounds) else {
        return nil
    }
    return bounds
}

func benchmarkDisplaySourceRect(
    _ globalBounds: CGRect,
    display: SCDisplay
) -> CGRect? {
    let sourceRect = globalBounds.offsetBy(
        dx: -display.frame.minX,
        dy: -display.frame.minY
    )
    guard sourceRect.origin.x.isFinite,
          sourceRect.origin.y.isFinite,
          sourceRect.width.isFinite,
          sourceRect.height.isFinite,
          sourceRect.minX >= 0,
          sourceRect.minY >= 0,
          sourceRect.width > 0,
          sourceRect.height > 0,
          sourceRect.maxX <= display.frame.width,
          sourceRect.maxY <= display.frame.height else {
        return nil
    }
    return sourceRect
}

func benchmarkStartScreenCaptureStream(
    displayID: CGDirectDisplayID,
    globalSourceRect: CGRect,
    operation: String,
    presentationBoundary: BenchmarkScreenCaptureBoundary? = nil
) async throws -> BenchmarkScreenCaptureSession {
    guard CGPreflightScreenCaptureAccess() else {
        throw BenchmarkFailure.message(
            "\(operation) requires ScreenCaptureKit authorization"
        )
    }
    // ScreenCaptureKit framework objects are intentionally non-Sendable.
    // Await their actor-bound async APIs directly instead of transferring them
    // through the benchmark deadline task.
    let shareableContent =
        try await SCShareableContent.excludingDesktopWindows(
            false,
            onScreenWindowsOnly: true
        )
    let discoveredDisplayIDs = shareableContent.displays
        .map { String($0.displayID) }
        .joined(separator: ",")
    guard let display = shareableContent.displays.first(where: {
        $0.displayID == displayID
    }) else {
        throw BenchmarkFailure.message(
            "\(operation) active capture display ID \(displayID) was missing "
                + "from ScreenCaptureKit (ScreenCaptureKit IDs=["
                + (discoveredDisplayIDs.isEmpty
                    ? "<none>"
                    : discoveredDisplayIDs)
                + "], CoreGraphics main ID=\(CGMainDisplayID()))"
        )
    }
    guard let sourceRect = benchmarkDisplaySourceRect(
        globalSourceRect,
        display: display
    ) else {
        throw BenchmarkFailure.message(
            "\(operation) could not map ROI \(globalSourceRect) onto capture "
                + "display ID \(display.displayID)"
        )
    }

    let configuration = SCStreamConfiguration()
    configuration.sourceRect = sourceRect
    let horizontalScale = Double(display.width) / display.frame.width
    let verticalScale = Double(display.height) / display.frame.height
    configuration.width = max(
        1,
        Int(ceil(sourceRect.width * horizontalScale))
    )
    configuration.height = max(
        1,
        Int(ceil(sourceRect.height * verticalScale))
    )
    configuration.minimumFrameInterval = CMTime(value: 1, timescale: 240)
    configuration.queueDepth = 5
    configuration.pixelFormat = kCVPixelFormatType_32BGRA
    configuration.showsCursor = false
    configuration.capturesAudio = false

    let filter = SCContentFilter(display: display, excludingWindows: [])
    let stream = SCStream(
        filter: filter,
        configuration: configuration,
        delegate: nil
    )
    let (frames, continuation) =
        AsyncStream<BenchmarkScreenCaptureFrame>.makeStream(
            bufferingPolicy: .bufferingNewest(3)
        )
    let output = BenchmarkScreenCaptureOutput(
        continuation: continuation,
        presentationBoundary: presentationBoundary
    )
    let sampleQueue = DispatchQueue(
        label: "org.srui.benchmark.screencapture.\(operation)"
    )
    try stream.addStreamOutput(
        output,
        type: .screen,
        sampleHandlerQueue: sampleQueue
    )
    do {
        try await stream.startCapture()
    } catch {
        try? stream.removeStreamOutput(output, type: .screen)
        output.finish()
        throw error
    }
    return BenchmarkScreenCaptureSession(
        frames: frames,
        stream: stream,
        output: output,
        sampleQueue: sampleQueue
    )
}

func benchmarkAwaitFirstCompleteFrame(
    from frames: AsyncStream<BenchmarkScreenCaptureFrame>,
    operation: String,
    afterDisplayTime: UInt64? = nil,
    timeout: Duration = .seconds(10)
) async throws -> BenchmarkScreenCaptureFrame {
    try await withBenchmarkDeadline(operation, timeout: timeout) {
        for await frame in frames {
            if let afterDisplayTime,
               frame.displayTime <= afterDisplayTime {
                continue
            }
            return frame
        }
        throw BenchmarkFailure.message(
            "\(operation) ended before a complete post-cutoff "
                + "ScreenCaptureKit frame"
        )
    }
}
let benchmarkStreamCaptureChannelTolerance = 2
let benchmarkScreenshotCaptureChannelTolerance = 5

struct BenchmarkCompositedDeltaEvidence: Sendable {
    let comparedPixelCount: Int
    let materiallyDifferentPixelCount: Int
    let maximumChannelDelta: Int
    let channelTolerance: Int

    var requiredMaterialPixelCount: Int {
        8
    }
}

func benchmarkRGBA8Bytes(_ image: CGImage) -> [UInt8]? {
    guard image.width > 0, image.height > 0 else { return nil }
    let bytesPerRow = image.width * 4
    let (byteCount, overflow) = bytesPerRow.multipliedReportingOverflow(
        by: image.height
    )
    guard overflow == false, byteCount > 0 else { return nil }

    var bytes = [UInt8](repeating: 0, count: byteCount)
    let drewImage = bytes.withUnsafeMutableBytes { storage -> Bool in
        guard let baseAddress = storage.baseAddress,
              let context = CGContext(
                  data: baseAddress,
                  width: image.width,
                  height: image.height,
                  bitsPerComponent: 8,
                  bytesPerRow: bytesPerRow,
                  space: CGColorSpace(name: CGColorSpace.sRGB)
                      ?? CGColorSpaceCreateDeviceRGB(),
                  bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue
                      | CGImageAlphaInfo.premultipliedLast.rawValue
              ) else {
            return false
        }
        context.interpolationQuality = .none
        context.draw(
            image,
            in: CGRect(
                x: 0,
                y: 0,
                width: image.width,
                height: image.height
            )
        )
        return true
    }
    return drewImage ? bytes : nil
}

func benchmarkCompositedDeltaEvidence(
    _ lhsBytes: [UInt8],
    _ rhsBytes: [UInt8],
    normalization: BenchmarkCompositedContentNormalization,
    channelTolerance: Int
) -> BenchmarkCompositedDeltaEvidence? {
    let (bytesPerRow, rowOverflow) =
        normalization.pixelWidth.multipliedReportingOverflow(by: 4)
    let (byteCount, sizeOverflow) =
        bytesPerRow.multipliedReportingOverflow(
            by: normalization.pixelHeight
        )
    guard channelTolerance >= 0,
          rowOverflow == false,
          sizeOverflow == false,
          byteCount > 0,
          lhsBytes.count == byteCount,
          rhsBytes.count == byteCount else {
        return nil
    }

    var comparedPixelCount = 0
    var materiallyDifferentPixelCount = 0
    var maximumChannelDelta = 0
    for y in 0..<normalization.pixelHeight {
        for x in 0..<normalization.pixelWidth {
            let isMasked =
                x >= normalization.markerMaskMinX
                && x < normalization.markerMaskMaxX
                && y >= normalization.markerMaskMinY
                && y < normalization.markerMaskMaxY
            if isMasked {
                continue
            }
            comparedPixelCount += 1
            let offset = y * bytesPerRow + x * 4
            var pixelIsMateriallyDifferent = false
            for channel in 0..<4 {
                let delta = abs(
                    Int(lhsBytes[offset + channel])
                        - Int(rhsBytes[offset + channel])
                )
                maximumChannelDelta = max(maximumChannelDelta, delta)
                if delta > channelTolerance {
                    pixelIsMateriallyDifferent = true
                }
            }
            if pixelIsMateriallyDifferent {
                materiallyDifferentPixelCount += 1
            }
        }
    }
    guard comparedPixelCount > 0 else { return nil }
    return BenchmarkCompositedDeltaEvidence(
        comparedPixelCount: comparedPixelCount,
        materiallyDifferentPixelCount: materiallyDifferentPixelCount,
        maximumChannelDelta: maximumChannelDelta,
        channelTolerance: channelTolerance
    )
}

func benchmarkCompositedDeltaEvidence(
    _ lhs: BenchmarkCompositedContentEvidence,
    _ rhs: BenchmarkCompositedContentEvidence,
    normalization: BenchmarkCompositedContentNormalization,
    channelTolerance: Int
) -> BenchmarkCompositedDeltaEvidence? {
    guard lhs.pixelWidth == normalization.pixelWidth,
          lhs.pixelHeight == normalization.pixelHeight,
          rhs.pixelWidth == normalization.pixelWidth,
          rhs.pixelHeight == normalization.pixelHeight else {
        return nil
    }
    return benchmarkCompositedDeltaEvidence(
        lhs.normalizedRGBA8Pixels,
        rhs.normalizedRGBA8Pixels,
        normalization: normalization,
        channelTolerance: channelTolerance
    )
}

func benchmarkCompositedDeltaEvidence(
    _ lhs: CGImage,
    _ rhs: CGImage,
    normalization: BenchmarkCompositedContentNormalization,
    channelTolerance: Int
) -> BenchmarkCompositedDeltaEvidence? {
    guard lhs.width == rhs.width,
          lhs.height == rhs.height,
          lhs.width == normalization.pixelWidth,
          lhs.height == normalization.pixelHeight,
          let lhsBytes = benchmarkRGBA8Bytes(lhs),
          let rhsBytes = benchmarkRGBA8Bytes(rhs) else {
        return nil
    }
    return benchmarkCompositedDeltaEvidence(
        lhsBytes,
        rhsBytes,
        normalization: normalization,
        channelTolerance: channelTolerance
    )
}

@MainActor
func benchmarkCaptureRestoredTarget(
    window: NSWindow,
    screen: NSScreen,
    expectedWindowEvidence: BenchmarkWindowServerEvidence,
    targetBounds: CGRect,
    baselineImage: CGImage,
    normalization: BenchmarkCompositedContentNormalization
) async throws -> (
    capture: BenchmarkCompositedCaptureEvidence,
    delta: BenchmarkCompositedDeltaEvidence
) {
    let deadline = clock.now + .seconds(10)
    var lastDelta: BenchmarkCompositedDeltaEvidence?
    let policy = FrameAcceptancePolicy()
    while clock.now < deadline {
        let observedEvidence = exactWindowServerEvidence(
            for: window,
            on: screen
        )
        let evidence: BenchmarkWindowServerEvidence
        switch policy.acceptWindowEvidence(
            observedEvidence,
            expected: expectedWindowEvidence,
            intersectingWindow: observedEvidence.flatMap(
                windowServerNonzeroAlphaIntersectionAbove
            )
        ) {
        case let .success(accepted):
            evidence = accepted
        case let .failure(rejection):
            throw BenchmarkFailure.message(
                "restoration frame rejected: \(rejection.description)"
            )
        }
        let capture = try await captureCompositedClientContent(
            evidence: evidence,
            clientContentBounds: targetBounds,
            normalization: normalization
        )
        guard capture.content.hasNonblankContent,
              capture.content.hasNonuniformContent,
              let delta = benchmarkCompositedDeltaEvidence(
                  capture.image,
                  baselineImage,
                  normalization: normalization,
                  channelTolerance:
                      benchmarkScreenshotCaptureChannelTolerance
              ) else {
            throw BenchmarkFailure.message(
                "post-cleanup target screenshot was blank, uniform, or could "
                    + "not be normalized against its baseline"
            )
        }
        lastDelta = delta
        if delta.materiallyDifferentPixelCount == 0 {
            return (capture, delta)
        }
        await Task.yield()
    }
    throw BenchmarkFailure.message(
        "post-cleanup ScreenCaptureKit screenshots did not restore within the "
            + "explicit \(benchmarkScreenshotCaptureChannelTolerance)/255 "
            + "per-channel screenshot "
            + "tolerance; last_material_pixels="
            + "\(lastDelta?.materiallyDifferentPixelCount ?? -1) "
            + "last_max_channel_delta="
            + "\(lastDelta?.maximumChannelDelta ?? -1)/255"
    )
}