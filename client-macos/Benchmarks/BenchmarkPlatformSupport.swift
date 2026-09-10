import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

struct RendererProcessIdentity: Codable {
    let pid: Int32
    let birthUnixNanoseconds: UInt64
    let observedAliveThroughUnixNanoseconds: UInt64

    enum CodingKeys: String, CodingKey {
        case pid
        case birthUnixNanoseconds = "birth_unix_ns"
        case observedAliveThroughUnixNanoseconds = "observed_alive_through_unix_ns"
    }
}

struct RendererMeasurementInterval: Codable {
    let startedUnixNanoseconds: UInt64
    let endedUnixNanoseconds: UInt64
    let requiredAllocationPIDs: [Int32]

    enum CodingKeys: String, CodingKey {
        case startedUnixNanoseconds = "started_unix_ns"
        case endedUnixNanoseconds = "ended_unix_ns"
        case requiredAllocationPIDs = "required_allocation_pids"
    }
}

struct RendererProcessAttribution: Codable {
    let candidate: String
    let driverPID: Int32
    let hostPID: Int32
    let helperPIDs: [Int32]
    let processIdentities: [RendererProcessIdentity]
    let startedUnixNanoseconds: UInt64
    let endedUnixNanoseconds: UInt64
    let measurementIntervals: [RendererMeasurementInterval]
    let helperPIDSource: String

    enum CodingKeys: String, CodingKey {
        case candidate
        case driverPID = "driver_pid"
        case hostPID = "host_pid"
        case helperPIDs = "helper_pids"
        case processIdentities = "process_identities"
        case startedUnixNanoseconds = "started_unix_ns"
        case endedUnixNanoseconds = "ended_unix_ns"
        case measurementIntervals = "measurement_intervals"
        case helperPIDSource = "helper_pid_source"
    }
}

struct ProcessResourceSample {
    let cpuMilliseconds: Double
    let physicalFootprintMiB: Double
    let measuredPIDCount: Int
}

struct ProcessFootprintMeasurement {
    let peakPhysicalFootprintMiB: Double
    let sampleCount: Int
    let allTargetProcessesMeasured: Bool
}

/// Samples the simultaneous current footprint of an immutable exact-PID set. All shared state is
/// protected by `lock`; the unchecked conformance exists only because NSLock is not Sendable.
final class ProcessFootprintSampler: @unchecked Sendable {
    private let processIDs: [pid_t]
    private let queue = DispatchQueue(label: "org.srui.benchmark.footprint-sampler")
    private let timer: DispatchSourceTimer
    private let lock = NSLock()
    private var intervalStartUnixNanoseconds: UInt64?
    private var intervalEndUnixNanoseconds: UInt64?
    private var peakPhysicalFootprintMiB = 0.0
    private var sampleCount = 0
    private var allTargetProcessesMeasured = true
    private var timerCancelled = false

    init(processIDs: [pid_t]) {
        self.processIDs = processIDs
        timer = DispatchSource.makeTimerSource(queue: queue)
        timer.setEventHandler { [weak self] in
            self?.sample()
        }
        timer.schedule(
            deadline: .now(),
            repeating: .milliseconds(1),
            leeway: .microseconds(250)
        )
        timer.resume()
    }

    func begin(at startedUnixNanoseconds: UInt64) {
        lock.lock()
        intervalStartUnixNanoseconds = startedUnixNanoseconds
        intervalEndUnixNanoseconds = nil
        peakPhysicalFootprintMiB = 0
        sampleCount = 0
        allTargetProcessesMeasured = true
        lock.unlock()
        sample()
    }

    func sampleNow() {
        sample()
    }

    func finish(at endedUnixNanoseconds: UInt64) -> ProcessFootprintMeasurement {
        lock.lock()
        intervalEndUnixNanoseconds = endedUnixNanoseconds
        let shouldCancel = timerCancelled == false
        timerCancelled = true
        lock.unlock()
        if shouldCancel {
            timer.cancel()
            queue.sync {}
        }
        lock.lock()
        let result = ProcessFootprintMeasurement(
            peakPhysicalFootprintMiB: peakPhysicalFootprintMiB,
            sampleCount: sampleCount,
            allTargetProcessesMeasured: allTargetProcessesMeasured
        )
        lock.unlock()
        return result
    }

    func cancel() {
        lock.lock()
        let shouldCancel = timerCancelled == false
        timerCancelled = true
        lock.unlock()
        if shouldCancel {
            timer.cancel()
            queue.sync {}
        }
    }

    private func sample() {
        let sampledStartedUnixNanoseconds = benchmarkWallClockNanoseconds()
        let resources = benchmarkProcessResourceSample(pids: processIDs)
        let sampledEndedUnixNanoseconds = benchmarkWallClockNanoseconds()
        lock.lock()
        defer { lock.unlock() }
        guard let intervalStartUnixNanoseconds,
              sampledStartedUnixNanoseconds >= intervalStartUnixNanoseconds,
              intervalEndUnixNanoseconds.map({
                  sampledEndedUnixNanoseconds <= $0
              }) ?? true else {
            return
        }
        sampleCount += 1
        peakPhysicalFootprintMiB = max(
            peakPhysicalFootprintMiB,
            resources.physicalFootprintMiB
        )
        allTargetProcessesMeasured = allTargetProcessesMeasured
            && resources.measuredPIDCount == processIDs.count
    }
}

struct BenchmarkVisibilityMarkerColor: Sendable {
    let red: UInt8
    let green: UInt8
    let blue: UInt8

    var appKitColor: NSColor {
        NSColor(
            srgbRed: CGFloat(red) / 255,
            green: CGFloat(green) / 255,
            blue: CGFloat(blue) / 255,
            alpha: 1
        )
    }
}

private let benchmarkVisibilityMarkerPalette = [
    BenchmarkVisibilityMarkerColor(red: 251, green: 17, blue: 113),
    BenchmarkVisibilityMarkerColor(red: 19, green: 239, blue: 173),
    BenchmarkVisibilityMarkerColor(red: 67, green: 43, blue: 251),
    BenchmarkVisibilityMarkerColor(red: 247, green: 211, blue: 23),
    BenchmarkVisibilityMarkerColor(red: 17, green: 181, blue: 251),
]

@MainActor
final class BenchmarkDrawCompletionProbe: NSView {
    static let markerBlockSize: CGFloat = 8

    private(set) var drawCount = 0
    private(set) var visibilityMarkerColors = [BenchmarkVisibilityMarkerColor]()
    private var markerRotation = 0

    override var isOpaque: Bool { false }

    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }

    func prepareVisibilityMarker() {
        markerRotation = (markerRotation + 1) % benchmarkVisibilityMarkerPalette.count
        visibilityMarkerColors = benchmarkVisibilityMarkerPalette.indices.map { index in
            benchmarkVisibilityMarkerPalette[
                (index + markerRotation) % benchmarkVisibilityMarkerPalette.count
            ]
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        drawCount += 1
        for (index, markerColor) in visibilityMarkerColors.enumerated() {
            markerColor.appKitColor.setFill()
            NSRect(
                x: 4 + CGFloat(index) * Self.markerBlockSize,
                y: 4,
                width: Self.markerBlockSize,
                height: Self.markerBlockSize
            ).fill()
        }
    }
}

struct BenchmarkCompositedContentEvidence: Sendable, Equatable {
    let normalizedFingerprintSHA256: String
    let pixelWidth: Int
    let pixelHeight: Int
    let unmaskedPixelCount: Int
    let distinctQuantizedColorCount: Int
    let nonDominantPixelCount: Int
    let hasNonblankContent: Bool
    let hasNonuniformContent: Bool
}

struct BenchmarkCompositedContentNormalization: Sendable, Equatable {
    let pixelWidth: Int
    let pixelHeight: Int
    let markerMaskMinX: Int
    let markerMaskMaxX: Int
    let markerMaskMinY: Int
    let markerMaskMaxY: Int
}

struct OnScreenPaintObservation: Sendable {
    let crossedDisplayRefresh: Bool
    let captureAuthorization: Bool
    let pixelCaptureVerified: Bool
    let presentedAt: ContinuousClock.Instant
    let visibilityProvenance: String
    let compositedContentEvidence: BenchmarkCompositedContentEvidence?
    let compositedContentNormalization:
        BenchmarkCompositedContentNormalization?

    init(
        crossedDisplayRefresh: Bool,
        captureAuthorization: Bool,
        pixelCaptureVerified: Bool,
        presentedAt: ContinuousClock.Instant = clock.now,
        visibilityProvenance: String = "caller_supplied_offscreen_or_aggregate",
        compositedContentEvidence: BenchmarkCompositedContentEvidence? = nil,
        compositedContentNormalization:
            BenchmarkCompositedContentNormalization? = nil
    ) {
        self.crossedDisplayRefresh = crossedDisplayRefresh
        self.captureAuthorization = captureAuthorization
        self.pixelCaptureVerified = pixelCaptureVerified
        self.presentedAt = presentedAt
        self.visibilityProvenance = visibilityProvenance
        self.compositedContentEvidence = compositedContentEvidence
        self.compositedContentNormalization = compositedContentNormalization
    }
}
func benchmarkProcessResourceSample(pids: [pid_t]) -> ProcessResourceSample {
    var cpuNanoseconds: UInt64 = 0
    var footprintBytes: UInt64 = 0
    var measured = 0
    for pid in pids where pid > 0 {
        var usage = rusage_info_v4()
        let status = withUnsafeMutablePointer(to: &usage) { pointer in
            pointer.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) {
                proc_pid_rusage(pid, RUSAGE_INFO_V4, $0)
            }
        }
        guard status == 0 else { continue }
        measured += 1
        cpuNanoseconds &+= usage.ri_user_time
        cpuNanoseconds &+= usage.ri_system_time
        footprintBytes &+= usage.ri_phys_footprint
    }
    return ProcessResourceSample(
        cpuMilliseconds: Double(cpuNanoseconds) / 1_000_000.0,
        physicalFootprintMiB: Double(footprintBytes) / 1_048_576.0,
        measuredPIDCount: measured
    )
}

func benchmarkWallClockNanoseconds() -> UInt64 {
    var value = timespec()
    clock_gettime(CLOCK_REALTIME, &value)
    return UInt64(value.tv_sec) * 1_000_000_000 + UInt64(value.tv_nsec)
}

func benchmarkProcessIdentity(pid: pid_t) -> RendererProcessIdentity? {
    guard pid > 0 else { return nil }
    let observedAliveThroughUnixNanoseconds = benchmarkWallClockNanoseconds()
    var info = proc_bsdinfo()
    let expectedSize = MemoryLayout<proc_bsdinfo>.stride
    let receivedSize = withUnsafeMutablePointer(to: &info) { pointer in
        proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, pointer, Int32(expectedSize))
    }
    guard receivedSize == Int32(expectedSize) else { return nil }
    let seconds = UInt64(info.pbi_start_tvsec)
    let microseconds = UInt64(info.pbi_start_tvusec)
    let (secondNanoseconds, secondsOverflow) = seconds.multipliedReportingOverflow(
        by: 1_000_000_000
    )
    let (microsecondNanoseconds, microsecondsOverflow) = microseconds
        .multipliedReportingOverflow(by: 1_000)
    let (birthUnixNanoseconds, additionOverflow) = secondNanoseconds
        .addingReportingOverflow(microsecondNanoseconds)
    guard secondsOverflow == false,
          microsecondsOverflow == false,
          additionOverflow == false,
          birthUnixNanoseconds > 0 else {
        return nil
    }
    return RendererProcessIdentity(
        pid: pid,
        birthUnixNanoseconds: birthUnixNanoseconds,
        observedAliveThroughUnixNanoseconds: observedAliveThroughUnixNanoseconds
    )
}

func benchmarkProcessMatchesIdentity(
    pid: pid_t,
    birthUnixNanoseconds: UInt64
) -> Bool {
    benchmarkProcessIdentity(pid: pid)?.birthUnixNanoseconds == birthUnixNanoseconds
}

final class BenchmarkParentLivenessWatchdog: @unchecked Sendable {
    private let parentPID: pid_t
    private let parentBirthUnixNanoseconds: UInt64
    private let candidatePID: pid_t
    private let candidateBirthUnixNanoseconds: UInt64
    private let condition = NSCondition()
    private let finished = DispatchSemaphore(value: 0)
    private var cancelled = false
    private var started = false
    private var thread: Thread?

    init(parentPID: pid_t, parentBirthUnixNanoseconds: UInt64) throws {
        guard let candidateIdentity = benchmarkProcessIdentity(pid: getpid()) else {
            throw BenchmarkFailure.message("candidate birth identity was unavailable")
        }
        self.parentPID = parentPID
        self.parentBirthUnixNanoseconds = parentBirthUnixNanoseconds
        self.candidatePID = getpid()
        self.candidateBirthUnixNanoseconds = candidateIdentity.birthUnixNanoseconds
    }

    func start() throws {
        guard getppid() == parentPID,
              benchmarkProcessMatchesIdentity(
                pid: parentPID,
                birthUnixNanoseconds: parentBirthUnixNanoseconds
              ),
              getpgrp() == candidatePID else {
            throw BenchmarkFailure.message(
                "candidate parent identity or process-group ownership was invalid"
            )
        }
        let worker = Thread { [weak self] in
            self?.run()
        }
        worker.name = "srui-benchmark-parent-watchdog"
        condition.lock()
        thread = worker
        started = true
        condition.unlock()
        worker.start()
        guard benchmarkProcessMatchesIdentity(
            pid: parentPID,
            birthUnixNanoseconds: parentBirthUnixNanoseconds
        ) else {
            terminateCandidateGroup()
        }
    }

    func cancel() {
        condition.lock()
        guard started else {
            condition.unlock()
            return
        }
        cancelled = true
        condition.broadcast()
        condition.unlock()
        _ = finished.wait(timeout: .now() + 2)
    }

    private func run() {
        defer { finished.signal() }
        while true {
            condition.lock()
            if cancelled {
                condition.unlock()
                return
            }
            _ = condition.wait(until: Date().addingTimeInterval(0.05))
            let shouldStop = cancelled
            condition.unlock()
            if shouldStop { return }
            guard benchmarkProcessMatchesIdentity(
                pid: parentPID,
                birthUnixNanoseconds: parentBirthUnixNanoseconds
            ) else {
                terminateCandidateGroup()
            }
        }
    }

    private func terminateCandidateGroup() -> Never {
        guard getpid() == candidatePID,
              getpgrp() == candidatePID,
              benchmarkProcessMatchesIdentity(
                pid: candidatePID,
                birthUnixNanoseconds: candidateBirthUnixNanoseconds
              ) else {
            Darwin._exit(74)
        }
        _ = Darwin.kill(-candidatePID, SIGKILL)
        Darwin._exit(75)
    }
}

struct LocalFrameBudgetObservation {
    let milliseconds: Double
    let source: String
}

@MainActor
func benchmarkLocalFrameBudget(fullPaint: Bool) -> LocalFrameBudgetObservation {
    let synthetic = LocalFrameBudgetObservation(
        milliseconds: 1_000.0 / 60.0,
        source: "configured synthetic 60Hz smoke renderer"
    )
    guard fullPaint else { return synthetic }
    guard let screen = NSScreen.main else {
        return LocalFrameBudgetObservation(
            milliseconds: synthetic.milliseconds,
            source: "60Hz fallback because NSScreen.main was unavailable"
        )
    }
    if let screenNumber = screen.deviceDescription[
        NSDeviceDescriptionKey("NSScreenNumber")
    ] as? NSNumber,
       let mode = CGDisplayCopyDisplayMode(CGDirectDisplayID(screenNumber.uint32Value)),
       mode.refreshRate > 0 {
        return LocalFrameBudgetObservation(
            milliseconds: 1_000.0 / mode.refreshRate,
            source: "CGDisplayMode.refreshRate for the benchmark NSScreen"
        )
    }
    if screen.maximumFramesPerSecond > 0 {
        return LocalFrameBudgetObservation(
            milliseconds: 1_000.0 / Double(screen.maximumFramesPerSecond),
            source: "NSScreen.maximumFramesPerSecond fallback because the display mode did not report a refresh rate"
        )
    }
    return LocalFrameBudgetObservation(
        milliseconds: synthetic.milliseconds,
        source: "60Hz fallback because the display cadence was unavailable"
    )
}

private func withBenchmarkDeadline<T: Sendable>(
    _ operation: String,
    timeout: Duration = .seconds(10),
    body: @escaping @Sendable () async throws -> T
) async throws -> T {
    let (stream, continuation) = AsyncThrowingStream<T, any Error>.makeStream(
        bufferingPolicy: .bufferingNewest(1)
    )
    let operationTask = Task {
        do {
            continuation.yield(try await body())
            continuation.finish()
        } catch {
            continuation.finish(throwing: error)
        }
    }
    let timeoutTask = Task {
        do {
            try await Task.sleep(for: timeout)
        } catch {
            return
        }
        continuation.finish(throwing: BenchmarkFailure.message("\(operation) timed out"))
    }
    defer {
        operationTask.cancel()
        timeoutTask.cancel()
        continuation.finish()
    }
    for try await value in stream {
        return value
    }
    try Task.checkCancellation()
    throw BenchmarkFailure.message("\(operation) produced no result")
}
private struct BenchmarkWindowServerEvidence: Equatable, Sendable {
    let windowID: CGWindowID
    let ownerPID: pid_t
    let layer: Int
    let bounds: CGRect
    let alpha: Double
    let displayID: CGDirectDisplayID
}

@MainActor
private func exactWindowServerEvidence(
    for window: NSWindow,
    on screen: NSScreen
) -> BenchmarkWindowServerEvidence? {
    let windowID = CGWindowID(window.windowNumber)
    guard windowID != 0,
          let screenNumber = screen.deviceDescription[
            NSDeviceDescriptionKey("NSScreenNumber")
          ] as? NSNumber,
          let entries = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements],
            kCGNullWindowID
          ) as? [[String: Any]] else {
        return nil
    }
    let matchingEntries = entries.filter {
        ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value == windowID
    }
    guard matchingEntries.count == 1,
          let entry = matchingEntries.first,
          let ownerPID = (entry[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value,
          ownerPID == getpid(),
          (entry[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue == true,
          let layer = (entry[kCGWindowLayer as String] as? NSNumber)?.intValue,
          layer == window.level.rawValue,
          let alpha = (entry[kCGWindowAlpha as String] as? NSNumber)?.doubleValue,
          alpha == 1,
          let boundsDictionary = entry[kCGWindowBounds as String] as? NSDictionary,
          let bounds = CGRect(
            dictionaryRepresentation: boundsDictionary as CFDictionary
          ),
          bounds.width.isFinite,
          bounds.height.isFinite,
          bounds.width > 0,
          bounds.height > 0 else {
        return nil
    }
    let displayID = CGDirectDisplayID(screenNumber.uint32Value)
    let displayBounds = CGDisplayBounds(displayID)
    guard displayBounds.intersection(bounds) == bounds else {
        return nil
    }
    return BenchmarkWindowServerEvidence(
        windowID: windowID,
        ownerPID: ownerPID,
        layer: layer,
        bounds: bounds,
        alpha: alpha,
        displayID: displayID
    )
}

private func windowServerReportsNoOpaqueIntersectionAbove(
    _ evidence: BenchmarkWindowServerEvidence
) -> Bool {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        return false
    }
    let targetIndices = entries.indices.filter { index in
        (entries[index][kCGWindowNumber as String] as? NSNumber)?.uint32Value
            == evidence.windowID
    }
    guard targetIndices.count == 1, let targetIndex = targetIndices.first else {
        return false
    }

    for index in entries.indices where index < targetIndex {
        let entry = entries[index]
        guard (entry[kCGWindowIsOnscreen as String] as? NSNumber)?
                .boolValue == true,
              let windowNumber = (
                  entry[kCGWindowNumber as String] as? NSNumber
              )?.uint32Value,
              let _ = (
                  entry[kCGWindowOwnerPID as String] as? NSNumber
              )?.int32Value,
              let _ = (
                  entry[kCGWindowLayer as String] as? NSNumber
              )?.intValue,
              let alpha = (
                  entry[kCGWindowAlpha as String] as? NSNumber
              )?.doubleValue,
              alpha.isFinite,
              alpha >= 0,
              let boundsDictionary = entry[
                  kCGWindowBounds as String
              ] as? NSDictionary,
              let bounds = CGRect(
                  dictionaryRepresentation: boundsDictionary as CFDictionary
              ),
              bounds.origin.x.isFinite,
              bounds.origin.y.isFinite,
              bounds.width.isFinite,
              bounds.height.isFinite,
              bounds.width >= 0,
              bounds.height >= 0 else {
            // The z-order proof is fail-closed: an ahead window whose required
            // identity/visibility/geometry fields are absent is not ignorable.
            return false
        }
        guard windowNumber != evidence.windowID else { return false }
        let intersection = bounds.intersection(evidence.bounds)
        if alpha > 0,
           intersection.isNull == false,
           intersection.width > 0,
           intersection.height > 0 {
            return false
        }
    }
    return true
}
private struct BenchmarkVisibilityMarkerMatch: Sendable {
    let x: Int
    let y: Int
    let step: Int
}

private struct BenchmarkCompositedCaptureEvidence: Sendable {
    let markerVisible: Bool
    let content: BenchmarkCompositedContentEvidence
    let normalization: BenchmarkCompositedContentNormalization
}

private func normalizedCompositedContentEvidence(
    _ image: CGImage,
    markerColors: [BenchmarkVisibilityMarkerColor]? = nil,
    normalization suppliedNormalization:
        BenchmarkCompositedContentNormalization? = nil
) -> BenchmarkCompositedCaptureEvidence? {
    guard image.width > 0, image.height > 0 else { return nil }
    let bytesPerRow = image.width * 4
    let (byteCount, overflow) = bytesPerRow.multipliedReportingOverflow(
        by: image.height
    )
    guard overflow == false, byteCount > 0 else { return nil }

    let pixels = UnsafeMutablePointer<UInt8>.allocate(capacity: byteCount)
    pixels.initialize(repeating: 0, count: byteCount)
    defer {
        pixels.deinitialize(count: byteCount)
        pixels.deallocate()
    }
    guard let context = CGContext(
        data: pixels,
        width: image.width,
        height: image.height,
        bitsPerComponent: 8,
        bytesPerRow: bytesPerRow,
        space: CGColorSpace(name: CGColorSpace.sRGB)
            ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue
            | CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        return nil
    }
    context.interpolationQuality = .none
    context.draw(
        image,
        in: CGRect(x: 0, y: 0, width: image.width, height: image.height)
    )

    let resolvedNormalization: BenchmarkCompositedContentNormalization
    let markerVisible: Bool
    if let suppliedNormalization {
        guard suppliedNormalization.pixelWidth == image.width,
              suppliedNormalization.pixelHeight == image.height else {
            return nil
        }
        resolvedNormalization = suppliedNormalization
        markerVisible = false
    } else {
        guard let markerColors, markerColors.count >= 3 else { return nil }
        let tolerance = 32
        func matches(
            x: Int,
            y: Int,
            color: BenchmarkVisibilityMarkerColor
        ) -> Bool {
            let offset = y * bytesPerRow + x * 4
            return abs(Int(pixels[offset]) - Int(color.red)) <= tolerance
                && abs(Int(pixels[offset + 1]) - Int(color.green)) <= tolerance
                && abs(Int(pixels[offset + 2]) - Int(color.blue)) <= tolerance
                && pixels[offset + 3] >= 224
        }

        var markerMatch: BenchmarkVisibilityMarkerMatch?
        markerSearch: for y in 0..<image.height {
            for x in 0..<image.width
            where matches(x: x, y: y, color: markerColors[0]) {
                let remaining = image.width - 1 - x
                let maximumStep = min(
                    64,
                    remaining / (markerColors.count - 1)
                )
                guard maximumStep >= 2 else { continue }
                for step in 2...maximumStep {
                    var complete = true
                    for index in 1..<markerColors.count where complete {
                        complete = matches(
                            x: x + index * step,
                            y: y,
                            color: markerColors[index]
                        )
                    }
                    if complete {
                        markerMatch = BenchmarkVisibilityMarkerMatch(
                            x: x,
                            y: y,
                            step: step
                        )
                        break markerSearch
                    }
                }
            }
        }
        guard let markerMatch else { return nil }

        // Mask the complete marker and a generous anti-aliasing fringe. The
        // passive observer reuses these exact coordinates, so removing the
        // preparation probe cannot manufacture a content change.
        resolvedNormalization = BenchmarkCompositedContentNormalization(
            pixelWidth: image.width,
            pixelHeight: image.height,
            markerMaskMinX: 0,
            markerMaskMaxX: min(
                image.width,
                max(
                    markerMatch.x
                        + (markerColors.count + 2) * markerMatch.step,
                    8 * markerMatch.step
                )
            ),
            markerMaskMinY: max(
                0,
                markerMatch.y - 4 * markerMatch.step
            ),
            markerMaskMaxY: min(
                image.height,
                markerMatch.y + 4 * markerMatch.step
            )
        )
        markerVisible = true
    }

    // The crop itself is client content only; window chrome, shadows, and
    // wallpaper never enter this normalized RGBA8 byte stream.
    var normalizedBytes = [UInt8]()
    let header = "\(image.width)x\(image.height):rgba8-srgb\n".utf8
    normalizedBytes.reserveCapacity(header.count + byteCount)
    normalizedBytes.append(contentsOf: header)
    var colorFrequencies = [UInt16: Int]()
    var unmaskedPixelCount = 0

    for y in 0..<image.height {
        for x in 0..<image.width {
            let offset = y * bytesPerRow + x * 4
            let isMarkerPixel =
                x >= resolvedNormalization.markerMaskMinX
                && x < resolvedNormalization.markerMaskMaxX
                && y >= resolvedNormalization.markerMaskMinY
                && y < resolvedNormalization.markerMaskMaxY
            if isMarkerPixel {
                normalizedBytes.append(contentsOf: [0, 0, 0, 0])
                continue
            }

            let red = pixels[offset]
            let green = pixels[offset + 1]
            let blue = pixels[offset + 2]
            let alpha = pixels[offset + 3]
            normalizedBytes.append(red)
            normalizedBytes.append(green)
            normalizedBytes.append(blue)
            normalizedBytes.append(alpha)
            unmaskedPixelCount += 1

            let quantized = UInt16(red >> 3) << 10
                | UInt16(green >> 3) << 5
                | UInt16(blue >> 3)
            colorFrequencies[quantized, default: 0] += 1
        }
    }

    let dominantPixelCount = colorFrequencies.values.max() ?? 0
    let nonDominantPixelCount = max(
        0,
        unmaskedPixelCount - dominantPixelCount
    )
    let minimumContentPixels = max(32, unmaskedPixelCount / 2_000)
    let contentEvidence = BenchmarkCompositedContentEvidence(
        normalizedFingerprintSHA256: digestHex(Data(normalizedBytes)),
        pixelWidth: image.width,
        pixelHeight: image.height,
        unmaskedPixelCount: unmaskedPixelCount,
        distinctQuantizedColorCount: colorFrequencies.count,
        nonDominantPixelCount: nonDominantPixelCount,
        hasNonblankContent: nonDominantPixelCount >= minimumContentPixels,
        hasNonuniformContent: colorFrequencies.count > 1
    )
    return BenchmarkCompositedCaptureEvidence(
        markerVisible: markerVisible,
        content: contentEvidence,
        normalization: resolvedNormalization
    )
}
@MainActor
private func exactClientContentBounds(
    for window: NSWindow,
    on screen: NSScreen,
    evidence: BenchmarkWindowServerEvidence
) -> CGRect? {
    guard let screenNumber = screen.deviceDescription[
        NSDeviceDescriptionKey("NSScreenNumber")
    ] as? NSNumber,
          CGDirectDisplayID(screenNumber.uint32Value) == evidence.displayID else {
        return nil
    }
    let displayBounds = CGDisplayBounds(evidence.displayID)
    let content = window.contentRect(forFrameRect: window.frame)

    // AppKit screen coordinates are bottom-left based; CoreGraphics and
    // ScreenCaptureKit display coordinates are top-left based. Convert the
    // exact AppKit client rect instead of inferring insets from kCGWindowBounds,
    // whose server-side rectangle may include the window shadow.
    var bounds = CGRect(
        x: displayBounds.minX + content.minX - screen.frame.minX,
        y: displayBounds.minY + screen.frame.maxY - content.maxY,
        width: content.width,
        height: content.height
    )
    // Remove the client-area edge so rounded frame pixels cannot admit
    // titlebar, shadow, or wallpaper into the normalized content evidence.
    bounds = bounds.insetBy(dx: 1, dy: 1)
    guard bounds.width > 0,
          bounds.height > 0,
          evidence.bounds.insetBy(dx: -0.5, dy: -0.5).contains(bounds) else {
        return nil
    }
    return bounds
}

private func captureCompositedClientContent(
    evidence: BenchmarkWindowServerEvidence,
    clientContentBounds: CGRect,
    markerColors: [BenchmarkVisibilityMarkerColor]? = nil,
    normalization: BenchmarkCompositedContentNormalization? = nil
) async throws -> BenchmarkCompositedCaptureEvidence {
    try await withBenchmarkDeadline(
        "ScreenCaptureKit composited client-content capture"
    ) {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false,
            onScreenWindowsOnly: true
        )
        guard let display = content.displays.first(where: {
            $0.displayID == evidence.displayID
        }) else {
            throw BenchmarkFailure.message(
                "ScreenCaptureKit could not resolve display \(evidence.displayID)"
            )
        }
        let sourceRect = clientContentBounds.offsetBy(
            dx: -display.frame.minX,
            dy: -display.frame.minY
        )
        guard sourceRect.minX >= 0,
              sourceRect.minY >= 0,
              sourceRect.maxX <= display.frame.width,
              sourceRect.maxY <= display.frame.height else {
            throw BenchmarkFailure.message(
                "exact client-content bounds escape the ScreenCaptureKit display"
            )
        }

        let filter = SCContentFilter(display: display, excludingWindows: [])
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
        configuration.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter,
            configuration: configuration
        )
        guard let capture = normalizedCompositedContentEvidence(
            image,
            markerColors: markerColors,
            normalization: normalization
        ) else {
            throw BenchmarkFailure.message(
                "composited client-content capture could not produce exact "
                    + "normalized content evidence"
            )
        }
        return capture
    }
}
private func captureAuthorizedWindowPixels(
    windowID: CGWindowID,
    pixelWidth: Int,
    pixelHeight: Int
) async throws -> Bool {
    try await withBenchmarkDeadline("ScreenCaptureKit authorized composited-window capture") {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false,
            onScreenWindowsOnly: true
        )
        guard let sharedWindow = content.windows.first(where: { $0.windowID == windowID }) else {
            throw BenchmarkFailure.message(
                "ScreenCaptureKit could not resolve benchmark window \(windowID)"
            )
        }
        let filter = SCContentFilter(desktopIndependentWindow: sharedWindow)
        let configuration = SCStreamConfiguration()
        configuration.width = pixelWidth
        configuration.height = pixelHeight
        configuration.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter,
            configuration: configuration
        )
        return image.width > 0 && image.height > 0
    }
}

@MainActor
func benchmarkVerifyAuthorizedWindowPixels(_ window: NSWindow) async throws -> Bool {
    guard CGPreflightScreenCaptureAccess() else { return false }
    let scale = window.screen?.backingScaleFactor ?? 1
    return try await captureAuthorizedWindowPixels(
        windowID: CGWindowID(window.windowNumber),
        pixelWidth: max(1, Int(window.frame.width * scale)),
        pixelHeight: max(1, Int(window.frame.height * scale))
    )
}

@MainActor
func benchmarkObserveOnScreenPaint(
    _ window: NSWindow,
    contentDrawProbe: BenchmarkDrawCompletionProbe? = nil,
    onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)? = nil
) async throws -> OnScreenPaintObservation {
    guard let screen = window.screen ?? NSScreen.main,
          let contentView = window.contentView else {
        throw BenchmarkFailure.message(
            "benchmark window has no display or drawable client content"
        )
    }

    let ownedDrawProbe: BenchmarkDrawCompletionProbe?
    let drawProbe: BenchmarkDrawCompletionProbe
    if let contentDrawProbe {
        ownedDrawProbe = nil
        drawProbe = contentDrawProbe
    } else {
        let probe = BenchmarkDrawCompletionProbe(frame: contentView.bounds)
        probe.autoresizingMask = [.width, .height]
        contentView.addSubview(probe, positioned: .above, relativeTo: nil)
        ownedDrawProbe = probe
        drawProbe = probe
    }
    defer {
        ownedDrawProbe?.removeFromSuperview()
    }

    // This compatibility helper is an explicit hidden-backing-store -> visible
    // redraw proof. Measured §31.1 code uses the action-wrapping API directly,
    // so capture setup is outside its timing and a later marker redraw is never
    // mislabeled as the production DOM/SRUI presentation boundary.
    if window.isVisible {
        window.orderOut(nil)
        let hiddenDeadline = Date().addingTimeInterval(2)
        while exactWindowServerEvidence(for: window, on: screen) != nil,
              Date() < hiddenDeadline {
            try await Task.sleep(for: .milliseconds(1))
        }
        guard window.isVisible == false,
              exactWindowServerEvidence(for: window, on: screen) == nil else {
            throw BenchmarkFailure.message(
                "explicit redraw preparation could not hide its prior window"
            )
        }
    }

    let drawCountBeforeSubmission = drawProbe.drawCount
    let measurement = try await benchmarkMeasureExplicitCompositedPaint(
        on: screen,
        action: {
            drawProbe.prepareVisibilityMarker()
            contentView.layoutSubtreeIfNeeded()
            contentView.needsDisplay = true
            drawProbe.needsDisplay = true
            window.displayIfNeeded()
            CATransaction.flush()
            return BenchmarkExplicitPaintTarget(
                window: window,
                targetView: contentView
            )
        },
        onPresented: onPresented
    )
    guard drawProbe.drawCount > drawCountBeforeSubmission else {
        throw BenchmarkFailure.message(
            "explicit content draw marker did not complete before presentation"
        )
    }
    return measurement.observation
}

struct BenchmarkPassivePresentationMeasurement: Sendable {
    let actionStartedAt: ContinuousClock.Instant
    let actionCompletedAt: ContinuousClock.Instant
    let actionStartedMachTicks: UInt64
    let acceptedDisplayMachTicks: UInt64
    let presentationLatencyMilliseconds: Double
    let observation: OnScreenPaintObservation
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

private enum BenchmarkScreenCaptureFrameDisposition {
    case continuous
    case baseline
    case drop
    case postArmCandidate
}

private final class BenchmarkScreenCaptureBoundary:
    @unchecked Sendable
{
    private enum State {
        case needsBaseline
        case waitingForArm
        case armed(UInt64)
        case accepted
    }

    private let lock = NSLock()
    private let onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)?
    private var state = State.needsBaseline

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

private struct BenchmarkScreenCaptureFrame: @unchecked Sendable {
    let displayTime: UInt64
    let receivedAt: ContinuousClock.Instant
    let receivedUnixNanoseconds: UInt64
    let isPostArmCandidate: Bool
    let image: CGImage
}

private final class BenchmarkScreenCaptureOutput:
    NSObject,
    SCStreamOutput,
    @unchecked Sendable
{
    private let continuation:
        AsyncStream<BenchmarkScreenCaptureFrame>.Continuation
    private let presentationBoundary: BenchmarkScreenCaptureBoundary?

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

private final class BenchmarkScreenCaptureSession: @unchecked Sendable {
    let frames: AsyncStream<BenchmarkScreenCaptureFrame>

    private let stream: SCStream
    private let output: BenchmarkScreenCaptureOutput
    private let sampleQueue: DispatchQueue
    private let lock = NSLock()
    private var stopped = false

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

private struct BenchmarkOwnedMenuFrameProof: Sendable {
    let windowEvidence: BenchmarkWindowServerEvidence
    let contentEvidence: BenchmarkCompositedContentEvidence
    let normalization: BenchmarkCompositedContentNormalization
    let presentedAt: ContinuousClock.Instant
    let acceptedDisplayMachTicks: UInt64
}

private final class BenchmarkMenuCancellationTarget:
    NSObject,
    @unchecked Sendable
{
    private weak var menu: NSMenu?

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
    @objc private func cancelMenu(_ ignored: Any?) {
        menu?.cancelTrackingWithoutAnimation()
    }
}

private func benchmarkZeroMaskNormalization(
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

private func benchmarkNormalizedFrameEvidence(
    _ frame: BenchmarkScreenCaptureFrame
) -> BenchmarkCompositedCaptureEvidence? {
    normalizedCompositedContentEvidence(
        frame.image,
        normalization: benchmarkZeroMaskNormalization(for: frame.image)
    )
}

@MainActor
private func exactTargetViewBounds(
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

private func benchmarkDisplaySourceRect(
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

private func benchmarkStartScreenCaptureStream(
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

private func benchmarkAwaitFirstCompleteFrame(
    from frames: AsyncStream<BenchmarkScreenCaptureFrame>,
    operation: String
) async throws -> BenchmarkScreenCaptureFrame {
    try await withBenchmarkDeadline(operation) {
        for await frame in frames {
            return frame
        }
        throw BenchmarkFailure.message(
            "\(operation) ended before a complete ScreenCaptureKit frame"
        )
    }
}

private func benchmarkAwaitChangedTargetFrame(
    from frames: AsyncStream<BenchmarkScreenCaptureFrame>,
    afterDisplayTime: UInt64,
    afterReceivedAt: ContinuousClock.Instant,
    baselineFingerprint: String,
    baselineNormalization: BenchmarkCompositedContentNormalization
) async throws -> (
    frame: BenchmarkScreenCaptureFrame,
    capture: BenchmarkCompositedCaptureEvidence
) {
    try await withBenchmarkDeadline(
        "post-action target-ROI composited change"
    ) {
        for await frame in frames {
            guard frame.displayTime > afterDisplayTime,
                  frame.receivedAt >= afterReceivedAt,
                  let capture = benchmarkNormalizedFrameEvidence(frame),
                  capture.normalization == baselineNormalization,
                  capture.content.hasNonblankContent,
                  capture.content.hasNonuniformContent,
                  capture.content.normalizedFingerprintSHA256
                      != baselineFingerprint else {
                continue
            }
            return (frame, capture)
        }
        throw BenchmarkFailure.message(
            "ScreenCaptureKit stream ended before the target ROI changed"
        )
    }
}

struct BenchmarkExplicitPaintTarget {
    let window: NSWindow
    let targetView: NSView
}

@MainActor
func benchmarkMeasureExplicitCompositedPaint(
    on screen: NSScreen,
    onActionStarting: (@MainActor () throws -> Void)? = nil,
    action:
        @escaping @MainActor () async throws -> BenchmarkExplicitPaintTarget,
    onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)? = nil
) async throws -> BenchmarkPassivePresentationMeasurement {
    guard let screenNumber = screen.deviceDescription[
        NSDeviceDescriptionKey("NSScreenNumber")
    ] as? NSNumber else {
        throw BenchmarkFailure.message(
            "explicit paint screen has no CoreGraphics display identity"
        )
    }
    let displayID = CGDirectDisplayID(screenNumber.uint32Value)
    let displayBounds = CGDisplayBounds(displayID)
    let presentationBoundary = BenchmarkScreenCaptureBoundary(
        onPresented: onPresented
    )
    let captureSession = try await benchmarkStartScreenCaptureStream(
        displayID: displayID,
        globalSourceRect: displayBounds,
        operation: "explicit full-display paint capture",
        presentationBoundary: presentationBoundary
    )

    do {
        let baselineFrame = try await benchmarkAwaitFirstCompleteFrame(
            from: captureSession.frames,
            operation: "explicit pre-action display baseline"
        )
        try onActionStarting?()
        let actionStartedMachTicks = mach_absolute_time()
        let actionStartedAt = clock.now
        let target = try await action()
        let actionCompletedAt = clock.now

        let targetScreen = target.window.screen ?? screen
        guard target.window.isVisible == false,
              target.targetView.window === target.window,
              let targetScreenNumber = targetScreen.deviceDescription[
                  NSDeviceDescriptionKey("NSScreenNumber")
              ] as? NSNumber,
              CGDirectDisplayID(targetScreenNumber.uint32Value) == displayID else {
            throw BenchmarkFailure.message(
                "explicit paint action must return a hidden target attached to "
                    + "the prepared capture display"
            )
        }

        let application = NSApplication.shared
        application.activate()
        let usesAppKitVisiblePath = application.isActive
        target.window.animationBehavior = .none
        if usesAppKitVisiblePath == false {
            target.window.level = .screenSaver
        }
        target.window.contentView?.layoutSubtreeIfNeeded()

        // The target is hidden until this one submission. Arm immediately
        // before ordering so the first complete frame can be the boundary.
        // A refresh that races ahead of the order remains fail-closed because
        // its exact target crop will equal the pre-action baseline.
        let submissionDisplayTime = mach_absolute_time()
        presentationBoundary.arm(
            afterDisplayTime: submissionDisplayTime
        )
        target.window.makeKeyAndOrderFront(nil)
        target.window.orderFrontRegardless()
        CATransaction.flush()

        typealias AcceptedCandidate = (
            frame: BenchmarkScreenCaptureFrame,
            evidence: BenchmarkWindowServerEvidence,
            targetBounds: CGRect,
            targetCapture: BenchmarkCompositedCaptureEvidence
        )
        var acceptedCandidate: AcceptedCandidate?
        var rejectedCandidateCount = 0
        var lastRejectedCondition =
            "condition 0/24: no post-cutoff complete frame was received"

        // Finish the frame stream at the deadline so the MainActor can retain
        // exact AppKit/WindowServer validation inside this sequential loop.
        // The enclosing catch then stops SCStream before returning the failure.
        let presentationTimeoutTask = Task {
            do {
                try await Task.sleep(for: .seconds(10))
            } catch {
                return
            }
            captureSession.finishFrames()
        }
        defer {
            presentationTimeoutTask.cancel()
        }

        candidateLoop:
        for await candidateFrame in captureSession.frames
            where candidateFrame.isPostArmCandidate
        {
            rejectedCandidateCount += 1
            let diagnosticSuffix =
                " [candidate \(rejectedCandidateCount), displayTime "
                + "\(candidateFrame.displayTime)]"

            guard candidateFrame.displayTime > submissionDisplayTime else {
                lastRejectedCondition =
                    "condition 1/24: displayTime was not after the arm cutoff"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard candidateFrame.receivedAt >= actionCompletedAt else {
                lastRejectedCondition =
                    "condition 2/24: callback receipt preceded action completion"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard candidateFrame.image.width == baselineFrame.image.width,
                  candidateFrame.image.height == baselineFrame.image.height else {
                lastRejectedCondition =
                    "condition 3/24: frame dimensions differed from baseline"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard target.window.isVisible else {
                lastRejectedCondition =
                    "condition 4/24: target window was not AppKit-visible"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard let evidence = exactWindowServerEvidence(
                for: target.window,
                on: screen
            ) else {
                lastRejectedCondition =
                    "condition 5/24: exact WindowServer target identity was absent"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard evidence.displayID == displayID else {
                lastRejectedCondition =
                    "condition 6/24: target appeared on a different display"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard let clientContentBounds = exactClientContentBounds(
                for: target.window,
                on: screen,
                evidence: evidence
            ) else {
                lastRejectedCondition =
                    "condition 7/24: exact client-content geometry was absent"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard let targetBounds = exactTargetViewBounds(
                target.targetView,
                in: target.window,
                on: screen,
                windowEvidence: evidence,
                clientContentBounds: clientContentBounds
            ) else {
                lastRejectedCondition =
                    "condition 8/24: exact target-view ROI was absent"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard windowServerReportsNoOpaqueIntersectionAbove(evidence) else {
                lastRejectedCondition =
                    "condition 9/24: another nontransparent window intersected "
                    + "the target in front" + diagnosticSuffix
                continue candidateLoop
            }
            if usesAppKitVisiblePath {
                guard target.window.occlusionState.contains(.visible) else {
                    lastRejectedCondition =
                        "condition 10/24: foreground target was not "
                        + "AppKit-visible" + diagnosticSuffix
                    continue candidateLoop
                }
            } else {
                guard target.window.occlusionState.rawValue != 0 else {
                    lastRejectedCondition =
                        "condition 10/24: fallback target was fully "
                        + "AppKit-occluded" + diagnosticSuffix
                    continue candidateLoop
                }
            }
            guard let baselineCrop = benchmarkCropDisplayFrame(
                baselineFrame.image,
                to: targetBounds,
                displayBounds: displayBounds
            ) else {
                lastRejectedCondition =
                    "condition 11/24: baseline target ROI could not be cropped"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard let targetCrop = benchmarkCropDisplayFrame(
                candidateFrame.image,
                to: targetBounds,
                displayBounds: displayBounds
            ) else {
                lastRejectedCondition =
                    "condition 12/24: candidate target ROI could not be cropped"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard let baselineCapture =
                normalizedCompositedContentEvidence(
                    baselineCrop,
                    normalization: benchmarkZeroMaskNormalization(
                        for: baselineCrop
                    )
                ) else {
                lastRejectedCondition =
                    "condition 13/24: baseline ROI normalization failed"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard let targetCapture =
                normalizedCompositedContentEvidence(
                    targetCrop,
                    normalization: benchmarkZeroMaskNormalization(
                        for: targetCrop
                    )
                ) else {
                lastRejectedCondition =
                    "condition 14/24: candidate ROI normalization failed"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard targetCapture.normalization
                    == baselineCapture.normalization else {
                lastRejectedCondition =
                    "condition 15/24: baseline and candidate normalization "
                    + "differed" + diagnosticSuffix
                continue candidateLoop
            }
            guard targetCapture.content.hasNonblankContent else {
                lastRejectedCondition =
                    "condition 16/24: candidate target ROI was blank"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard targetCapture.content.hasNonuniformContent else {
                lastRejectedCondition =
                    "condition 17/24: candidate target ROI was uniform"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard targetCapture.content.normalizedFingerprintSHA256
                    != baselineCapture.content.normalizedFingerprintSHA256 else {
                lastRejectedCondition =
                    "condition 18/24: candidate target ROI equaled baseline"
                    + diagnosticSuffix
                continue candidateLoop
            }

            // Recheck after hashing this same candidate frame. Any missing
            // identity/geometry/ROI field or nonzero-alpha overlap rejects the
            // candidate; the stream remains armed for the next complete frame.
            guard target.window.isVisible else {
                lastRejectedCondition =
                    "condition 19/24: target became hidden during verification"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard let recheckedEvidence = exactWindowServerEvidence(
                for: target.window,
                on: screen
            ),
                  recheckedEvidence == evidence else {
                lastRejectedCondition =
                    "condition 20/24: WindowServer identity or geometry changed "
                    + "during verification" + diagnosticSuffix
                continue candidateLoop
            }
            guard let recheckedClientContentBounds = exactClientContentBounds(
                for: target.window,
                on: screen,
                evidence: recheckedEvidence
            ),
                  recheckedClientContentBounds == clientContentBounds else {
                lastRejectedCondition =
                    "condition 21/24: client-content geometry changed during "
                    + "verification" + diagnosticSuffix
                continue candidateLoop
            }
            guard let recheckedTargetBounds = exactTargetViewBounds(
                target.targetView,
                in: target.window,
                on: screen,
                windowEvidence: recheckedEvidence,
                clientContentBounds: recheckedClientContentBounds
            ),
                  recheckedTargetBounds == targetBounds else {
                lastRejectedCondition =
                    "condition 22/24: target-view ROI changed during verification"
                    + diagnosticSuffix
                continue candidateLoop
            }
            guard windowServerReportsNoOpaqueIntersectionAbove(
                recheckedEvidence
            ) else {
                lastRejectedCondition =
                    "condition 23/24: target became intersected during "
                    + "verification" + diagnosticSuffix
                continue candidateLoop
            }
            guard presentationBoundary.accept(
                displayTime: candidateFrame.displayTime,
                receivedAt: candidateFrame.receivedAt,
                receivedUnixNanoseconds:
                    candidateFrame.receivedUnixNanoseconds
            ) else {
                lastRejectedCondition =
                    "condition 24/24: capture boundary rejected the proven frame"
                    + diagnosticSuffix
                continue candidateLoop
            }

            acceptedCandidate = (
                frame: candidateFrame,
                evidence: evidence,
                targetBounds: targetBounds,
                targetCapture: targetCapture
            )
            break candidateLoop
        }

        guard let acceptedCandidate else {
            throw BenchmarkFailure.message(
                "explicit composited presentation timed out or ended after "
                    + "\(rejectedCandidateCount) post-cutoff candidate(s); "
                    + "last rejected condition: \(lastRejectedCondition)"
            )
        }
        let candidateFrame = acceptedCandidate.frame
        let evidence = acceptedCandidate.evidence
        let targetBounds = acceptedCandidate.targetBounds
        let targetCapture = acceptedCandidate.targetCapture

        let visibilityProvenance = usesAppKitVisiblePath
            ? "screencapturekit_first_complete_target_frame_appkit_visible"
            : "screencapturekit_first_complete_target_frame_screen_saver_level"
        let observation = OnScreenPaintObservation(
            crossedDisplayRefresh: true,
            captureAuthorization: true,
            pixelCaptureVerified: true,
            presentedAt: candidateFrame.receivedAt,
            visibilityProvenance: visibilityProvenance,
            compositedContentEvidence: targetCapture.content,
            compositedContentNormalization: targetCapture.normalization
        )
        await captureSession.stop()
        benchmarkTrace(
            "explicit paint provenance=\(visibilityProvenance) "
                + "window=\(evidence.windowID) target=\(targetBounds) "
                + "fingerprint="
                + String(
                    targetCapture.content.normalizedFingerprintSHA256
                        .prefix(16)
                )
        )
        return BenchmarkPassivePresentationMeasurement(
            actionStartedAt: actionStartedAt,
            actionCompletedAt: actionCompletedAt,
            actionStartedMachTicks: actionStartedMachTicks,
            acceptedDisplayMachTicks: candidateFrame.displayTime,
            presentationLatencyMilliseconds:
                try benchmarkMachElapsedMilliseconds(
                    from: actionStartedMachTicks,
                    to: candidateFrame.displayTime
                ),
            observation: observation
        )
    } catch {
        await captureSession.stop()
        throw error
    }
}

@MainActor
private func benchmarkPrepareExactVisibleWindow(
    _ window: NSWindow
) async throws -> (
    screen: NSScreen,
    visibilityProvenance: String,
    evidence: BenchmarkWindowServerEvidence,
    clientContentBounds: CGRect
) {
    guard CGPreflightScreenCaptureAccess(),
          let screen = window.screen ?? NSScreen.main,
          window.contentView != nil else {
        throw BenchmarkFailure.message(
            "composited preparation requires capture authorization, a display, "
                + "and a client content view"
        )
    }

    func submitUntimedWindow() {
        NSApplication.shared.activate()
        window.makeKeyAndOrderFront(nil)
        window.orderFrontRegardless()
        window.contentView?.layoutSubtreeIfNeeded()
        window.contentView?.needsDisplay = true
        window.displayIfNeeded()
        CATransaction.flush()
    }

    func awaitExactEvidence() async throws -> BenchmarkWindowServerEvidence {
        let deadline = Date().addingTimeInterval(2)
        var evidence = exactWindowServerEvidence(for: window, on: screen)
        while (window.isVisible == false || evidence == nil), Date() < deadline {
            submitUntimedWindow()
            try await Task.sleep(for: .milliseconds(1))
            evidence = exactWindowServerEvidence(for: window, on: screen)
        }
        guard window.isVisible, let evidence else {
            throw BenchmarkFailure.message(
                "untimed preparation did not acquire exact WindowServer "
                    + "identity, on-screen state, layer, bounds, and display"
            )
        }
        return evidence
    }

    submitUntimedWindow()
    var evidence = try await awaitExactEvidence()
    let visibilityProvenance: String
    if window.occlusionState.contains(.visible) {
        visibilityProvenance = "appkit_occlusion_visible"
    } else {
        guard window.occlusionState.rawValue != 0 else {
            throw BenchmarkFailure.message(
                "AppKit reported the benchmark window fully occluded"
            )
        }
        // The command-line full benchmark cannot always become foreground on
        // modern macOS. Elevate only the fail-closed composited-evidence path;
        // the target ROI is still captured and every intersecting visible
        // nonzero-alpha window ahead is rejected below.
        window.level = .screenSaver
        submitUntimedWindow()
        evidence = try await awaitExactEvidence()
        visibilityProvenance =
            "screencapturekit_composited_baseline_screen_saver_level"
    }

    // WindowServer can publish opening-animation geometry before the exact
    // final client rectangle. Poll its own identity/geometry without forcing
    // any further draw; the first SCStream frame acquired by the caller is the
    // actual composited baseline.
    let stableGeometryDeadline = Date().addingTimeInterval(2)
    var clientContentBounds: CGRect?
    while clientContentBounds == nil, Date() < stableGeometryDeadline {
        guard let currentEvidence = exactWindowServerEvidence(
            for: window,
            on: screen
        ) else {
            throw BenchmarkFailure.message(
                "prepared window lost exact WindowServer evidence"
            )
        }
        evidence = currentEvidence
        clientContentBounds = exactClientContentBounds(
            for: window,
            on: screen,
            evidence: currentEvidence
        )
        if clientContentBounds == nil {
            try await Task.sleep(for: .milliseconds(1))
        }
    }
    guard let clientContentBounds,
          windowServerReportsNoOpaqueIntersectionAbove(evidence) else {
        throw BenchmarkFailure.message(
            "prepared window never reached exact final unobscured client geometry"
        )
    }
    return (
        screen,
        visibilityProvenance,
        evidence,
        clientContentBounds
    )
}

@MainActor
func benchmarkMeasurePassiveCompositedChange(
    _ window: NSWindow,
    targetView: NSView,
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

        guard let preActionEvidence = exactWindowServerEvidence(
            for: window,
            on: prepared.screen
        ),
              preActionEvidence == prepared.evidence,
              let preActionClientContentBounds = exactClientContentBounds(
                  for: window,
                  on: prepared.screen,
                  evidence: preActionEvidence
              ),
              preActionClientContentBounds == prepared.clientContentBounds,
              let preActionTargetBounds = exactTargetViewBounds(
                  targetView,
                  in: window,
                  on: prepared.screen,
                  windowEvidence: preActionEvidence,
                  clientContentBounds: preActionClientContentBounds
              ),
              preActionTargetBounds == targetBounds,
              windowServerReportsNoOpaqueIntersectionAbove(
                  preActionEvidence
              ) else {
            throw BenchmarkFailure.message(
                "target window identity, geometry, ROI, or unobscured state "
                    + "changed while acquiring its baseline"
            )
        }

        let actionStartedMachTicks = mach_absolute_time()
        let actionStartedAt = clock.now
        try await action()

        // Frames delivered while the action closure is running can represent an
        // intermediate state (notably a cadence batch). Arm the cutoff only
        // after the closure returns; timeout is safer than accepting a false
        // final decode-to-visible result.
        let actionCompletedAt = clock.now
        let postActionDisplayTime = mach_absolute_time()
        let changed = try await benchmarkAwaitChangedTargetFrame(
            from: captureSession.frames,
            afterDisplayTime: postActionDisplayTime,
            afterReceivedAt: actionCompletedAt,
            baselineFingerprint:
                baselineCapture.content.normalizedFingerprintSHA256,
            baselineNormalization: baselineCapture.normalization
        )

        guard let postActionEvidence = exactWindowServerEvidence(
            for: window,
            on: prepared.screen
        ),
              postActionEvidence == prepared.evidence,
              let postActionClientContentBounds = exactClientContentBounds(
                  for: window,
                  on: prepared.screen,
                  evidence: postActionEvidence
              ),
              postActionClientContentBounds == prepared.clientContentBounds,
              let postActionTargetBounds = exactTargetViewBounds(
                  targetView,
                  in: window,
                  on: prepared.screen,
                  windowEvidence: postActionEvidence,
                  clientContentBounds: postActionClientContentBounds
              ),
              postActionTargetBounds == targetBounds,
              windowServerReportsNoOpaqueIntersectionAbove(
                  postActionEvidence
              ) else {
            throw BenchmarkFailure.message(
                "accepted target frame changed exact window identity, geometry, "
                    + "ROI, display, or unobscured visibility"
            )
        }

        let provenance =
            "screencapturekit_same_complete_frame_target_roi_after_action_"
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
            observation: observation
        )
    } catch {
        await captureSession.stop()
        throw error
    }
}

private func benchmarkOnScreenOwnedWindowIDs() throws -> Set<CGWindowID> {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        throw BenchmarkFailure.message(
            "WindowServer did not provide the pre-menu owned-window inventory"
        )
    }

    var identifiers = Set<CGWindowID>()
    for entry in entries {
        guard let ownerPID = (
            entry[kCGWindowOwnerPID as String] as? NSNumber
        )?.int32Value else {
            throw BenchmarkFailure.message(
                "pre-menu WindowServer inventory omitted an owner PID"
            )
        }
        guard ownerPID == getpid() else { continue }
        guard (entry[kCGWindowIsOnscreen as String] as? NSNumber)?
                .boolValue == true,
              let identifier = (
                  entry[kCGWindowNumber as String] as? NSNumber
              )?.uint32Value,
              identifier != 0 else {
            throw BenchmarkFailure.message(
                "pre-menu owned window omitted exact on-screen identity"
            )
        }
        identifiers.insert(identifier)
    }
    return identifiers
}

private func benchmarkExactOwnedMenuWindowEvidence(
    excluding baselineWindowIDs: Set<CGWindowID>,
    displayID: CGDirectDisplayID,
    menuLayer: Int
) throws -> BenchmarkWindowServerEvidence? {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        throw BenchmarkFailure.message(
            "WindowServer did not provide post-menu window evidence"
        )
    }
    let displayBounds = CGDisplayBounds(displayID)
    var candidates = [BenchmarkWindowServerEvidence]()

    for entry in entries {
        guard let ownerPID = (
            entry[kCGWindowOwnerPID as String] as? NSNumber
        )?.int32Value else {
            throw BenchmarkFailure.message(
                "post-menu WindowServer inventory omitted an owner PID"
            )
        }
        guard ownerPID == getpid() else { continue }
        guard (entry[kCGWindowIsOnscreen as String] as? NSNumber)?
                .boolValue == true,
              let identifier = (
                  entry[kCGWindowNumber as String] as? NSNumber
              )?.uint32Value,
              identifier != 0,
              let layer = (
                  entry[kCGWindowLayer as String] as? NSNumber
              )?.intValue,
              let alpha = (
                  entry[kCGWindowAlpha as String] as? NSNumber
              )?.doubleValue,
              alpha.isFinite,
              alpha > 0,
              let boundsDictionary = entry[
                  kCGWindowBounds as String
              ] as? NSDictionary,
              let bounds = CGRect(
                  dictionaryRepresentation: boundsDictionary as CFDictionary
              ),
              bounds.origin.x.isFinite,
              bounds.origin.y.isFinite,
              bounds.width.isFinite,
              bounds.height.isFinite,
              bounds.width > 0,
              bounds.height > 0 else {
            throw BenchmarkFailure.message(
                "post-menu owned window omitted required visibility or geometry"
            )
        }
        guard baselineWindowIDs.contains(identifier) == false,
              layer == menuLayer else {
            continue
        }
        guard displayBounds.contains(bounds) else {
            throw BenchmarkFailure.message(
                "new owned menu window escaped the host display"
            )
        }
        candidates.append(
            BenchmarkWindowServerEvidence(
                windowID: identifier,
                ownerPID: ownerPID,
                layer: layer,
                bounds: bounds,
                alpha: alpha,
                displayID: displayID
            )
        )
    }

    guard candidates.count <= 1 else {
        throw BenchmarkFailure.message(
            "WindowServer exposed multiple new owned menu-level windows"
        )
    }
    return candidates.first
}

private func benchmarkRecheckOwnedMenuWindowEvidence(
    _ expected: BenchmarkWindowServerEvidence
) throws -> BenchmarkWindowServerEvidence? {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        throw BenchmarkFailure.message(
            "WindowServer did not provide the menu recheck"
        )
    }
    let matches = entries.filter {
        ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value
            == expected.windowID
    }
    guard matches.count == 1, let entry = matches.first else {
        return nil
    }
    guard let ownerPID = (
        entry[kCGWindowOwnerPID as String] as? NSNumber
    )?.int32Value,
          ownerPID == expected.ownerPID,
          (entry[kCGWindowIsOnscreen as String] as? NSNumber)?
            .boolValue == true,
          let layer = (
              entry[kCGWindowLayer as String] as? NSNumber
          )?.intValue,
          layer == expected.layer,
          let alpha = (
              entry[kCGWindowAlpha as String] as? NSNumber
          )?.doubleValue,
          alpha.isFinite,
          alpha > 0,
          let boundsDictionary = entry[
              kCGWindowBounds as String
          ] as? NSDictionary,
          let bounds = CGRect(
              dictionaryRepresentation: boundsDictionary as CFDictionary
          ),
          bounds.origin.x.isFinite,
          bounds.origin.y.isFinite,
          bounds.width.isFinite,
          bounds.height.isFinite,
          bounds.width > 0,
          bounds.height > 0,
          CGDisplayBounds(expected.displayID).contains(bounds) else {
        return nil
    }
    return BenchmarkWindowServerEvidence(
        windowID: expected.windowID,
        ownerPID: ownerPID,
        layer: layer,
        bounds: bounds,
        alpha: alpha,
        displayID: expected.displayID
    )
}

private func benchmarkCropDisplayFrame(
    _ image: CGImage,
    to globalBounds: CGRect,
    displayBounds: CGRect
) -> CGImage? {
    guard displayBounds.contains(globalBounds),
          displayBounds.width > 0,
          displayBounds.height > 0 else {
        return nil
    }
    let scaleX = CGFloat(image.width) / displayBounds.width
    let scaleY = CGFloat(image.height) / displayBounds.height
    let pixelBounds = CGRect(
        x: (globalBounds.minX - displayBounds.minX) * scaleX,
        y: (globalBounds.minY - displayBounds.minY) * scaleY,
        width: globalBounds.width * scaleX,
        height: globalBounds.height * scaleY
    ).integral
    let imageBounds = CGRect(
        x: 0,
        y: 0,
        width: image.width,
        height: image.height
    )
    guard pixelBounds.width > 0,
          pixelBounds.height > 0,
          imageBounds.contains(pixelBounds) else {
        return nil
    }
    return image.cropping(to: pixelBounds)
}

private func benchmarkAwaitOwnedMenuFrame(
    from frames: AsyncStream<BenchmarkScreenCaptureFrame>,
    baselineFrame: BenchmarkScreenCaptureFrame,
    baselineWindowIDs: Set<CGWindowID>,
    displayID: CGDirectDisplayID,
    menuLayer: Int,
    afterDisplayTime: UInt64,
    afterReceivedAt: ContinuousClock.Instant
) async throws -> BenchmarkOwnedMenuFrameProof {
    let displayBounds = CGDisplayBounds(displayID)
    return try await withBenchmarkDeadline(
        "owned menu composited presentation",
        timeout: .seconds(3)
    ) {
        for await frame in frames {
            guard frame.displayTime > afterDisplayTime,
                  frame.receivedAt >= afterReceivedAt else {
                continue
            }
            guard frame.image.width == baselineFrame.image.width,
                  frame.image.height == baselineFrame.image.height else {
                throw BenchmarkFailure.message(
                    "owned menu capture changed exact display pixel geometry"
                )
            }
            guard let candidate = try benchmarkExactOwnedMenuWindowEvidence(
                      excluding: baselineWindowIDs,
                      displayID: displayID,
                      menuLayer: menuLayer
                  ),
                  windowServerReportsNoOpaqueIntersectionAbove(candidate),
                  let baselineCrop = benchmarkCropDisplayFrame(
                      baselineFrame.image,
                      to: candidate.bounds,
                      displayBounds: displayBounds
                  ),
                  let menuCrop = benchmarkCropDisplayFrame(
                      frame.image,
                      to: candidate.bounds,
                      displayBounds: displayBounds
                  ),
                  let baselineCapture =
                      normalizedCompositedContentEvidence(
                          baselineCrop,
                          normalization: benchmarkZeroMaskNormalization(
                              for: baselineCrop
                          )
                      ),
                  let menuCapture =
                      normalizedCompositedContentEvidence(
                          menuCrop,
                          normalization: benchmarkZeroMaskNormalization(
                              for: menuCrop
                          )
                      ),
                  menuCapture.normalization
                      == baselineCapture.normalization,
                  menuCapture.content.hasNonblankContent,
                  menuCapture.content.hasNonuniformContent,
                  menuCapture.content.normalizedFingerprintSHA256
                      != baselineCapture.content.normalizedFingerprintSHA256
            else {
                continue
            }

            // Requery after hashing the accepted frame and before requesting
            // cancellation. Missing fields, any geometry/alpha change, or any
            // intersecting visible nonzero-alpha window ahead fails closed.
            guard let rechecked =
                try benchmarkRecheckOwnedMenuWindowEvidence(candidate),
                  rechecked == candidate,
                  windowServerReportsNoOpaqueIntersectionAbove(rechecked)
            else {
                throw BenchmarkFailure.message(
                    "owned menu identity, geometry, z-order, or visibility "
                        + "changed while verifying its accepted frame"
                )
            }
            return BenchmarkOwnedMenuFrameProof(
                windowEvidence: rechecked,
                contentEvidence: menuCapture.content,
                normalization: menuCapture.normalization,
                presentedAt: frame.receivedAt,
                acceptedDisplayMachTicks: frame.displayTime
            )
        }
        throw BenchmarkFailure.message(
            "ScreenCaptureKit ended before an exact owned menu frame"
        )
    }
}

@MainActor
func benchmarkMeasureOwnedMenuPresentation(
    _ menu: NSMenu,
    positioningItem: NSMenuItem?,
    at location: NSPoint,
    in view: NSView
) async throws -> BenchmarkMenuPresentationMeasurement {
    guard let window = view.window else {
        throw BenchmarkFailure.message(
            "menu benchmark host view is not attached to a window"
        )
    }
    let prepared = try await benchmarkPrepareExactVisibleWindow(window)
    if window.level == .screenSaver {
        // The fallback host must remain above ordinary app windows but below
        // AppKit's separate pop-up-menu window. Otherwise a layer-101 menu is
        // truthfully reported as covered by its own layer-1000 host.
        window.level = .statusBar
        window.orderFrontRegardless()
        CATransaction.flush()
        let hostLevelDeadline = Date().addingTimeInterval(2)
        var loweredHostEvidence = exactWindowServerEvidence(
            for: window,
            on: prepared.screen
        )
        while loweredHostEvidence == nil, Date() < hostLevelDeadline {
            try await Task.sleep(for: .milliseconds(1))
            loweredHostEvidence = exactWindowServerEvidence(
                for: window,
                on: prepared.screen
            )
        }
        guard let loweredHostEvidence,
              loweredHostEvidence.layer == NSWindow.Level.statusBar.rawValue,
              windowServerReportsNoOpaqueIntersectionAbove(
                  loweredHostEvidence
              ) else {
            throw BenchmarkFailure.message(
                "menu host could not establish an unobscured level below its "
                    + "owned pop-up window"
            )
        }
    }
    let baselineWindowIDs = try benchmarkOnScreenOwnedWindowIDs()
    let displayID = prepared.evidence.displayID
    let displayBounds = CGDisplayBounds(displayID)
    let captureSession = try await benchmarkStartScreenCaptureStream(
        displayID: displayID,
        globalSourceRect: displayBounds,
        operation: "owned menu full-display capture"
    )

    do {
        // The most recent complete pre-action display sample is retained so the
        // exact eventual menu rectangle can be compared byte-for-byte against
        // the same rectangle before the menu existed.
        let baselineFrame = try await benchmarkAwaitFirstCompleteFrame(
            from: captureSession.frames,
            operation: "owned menu pre-action display baseline"
        )
        let actionStartedMachTicks = mach_absolute_time()
        let actionStartedAt = clock.now
        let postActionDisplayTime = actionStartedMachTicks
        let menuLayer = NSWindow.Level.popUpMenu.rawValue
        let cancellationTarget = BenchmarkMenuCancellationTarget(menu: menu)
        let evidenceTask = Task.detached {
            do {
                let proof = try await benchmarkAwaitOwnedMenuFrame(
                    from: captureSession.frames,
                    baselineFrame: baselineFrame,
                    baselineWindowIDs: baselineWindowIDs,
                    displayID: displayID,
                    menuLayer: menuLayer,
                    afterDisplayTime: postActionDisplayTime,
                    afterReceivedAt: actionStartedAt
                )
                cancellationTarget.schedule()
                return proof
            } catch {
                cancellationTarget.schedule()
                throw error
            }
        }

        _ = menu.popUp(
            positioning: positioningItem,
            at: location,
            in: view
        )

        let proof: BenchmarkOwnedMenuFrameProof
        do {
            proof = try await evidenceTask.value
        } catch {
            evidenceTask.cancel()
            await captureSession.stop()
            throw error
        }
        await captureSession.stop()

        let provenance =
            "screencapturekit_same_complete_frame_new_owned_menu_window"
        let observation = OnScreenPaintObservation(
            crossedDisplayRefresh: true,
            captureAuthorization: true,
            pixelCaptureVerified: true,
            presentedAt: proof.presentedAt,
            visibilityProvenance: provenance,
            compositedContentEvidence: proof.contentEvidence,
            compositedContentNormalization: proof.normalization
        )
        benchmarkTrace(
            "menu paint provenance=\(provenance) "
                + "window=\(proof.windowEvidence.windowID) "
                + "bounds=\(proof.windowEvidence.bounds) "
                + "fingerprint="
                + String(
                    proof.contentEvidence.normalizedFingerprintSHA256
                        .prefix(16)
                )
        )
        return BenchmarkMenuPresentationMeasurement(
            actionStartedAt: actionStartedAt,
            actionStartedMachTicks: actionStartedMachTicks,
            acceptedDisplayMachTicks: proof.acceptedDisplayMachTicks,
            presentationLatencyMilliseconds:
                try benchmarkMachElapsedMilliseconds(
                    from: actionStartedMachTicks,
                    to: proof.acceptedDisplayMachTicks
                ),
            observation: observation,
            menuWindowID: proof.windowEvidence.windowID
        )
    } catch {
        await captureSession.stop()
        throw error
    }
}
@MainActor
private func diagnosticProcessID(
    _ object: NSObject,
    selectorName: String
) -> pid_t? {
    let selector = NSSelectorFromString(selectorName)
    guard object.responds(to: selector), let implementation = object.method(for: selector) else {
        return nil
    }
    typealias Getter = @convention(c) (AnyObject, Selector) -> pid_t
    let getter = unsafeBitCast(implementation, to: Getter.self)
    let value = getter(object, selector)
    return value > 0 ? value : nil
}

@MainActor
func benchmarkWebKitWebContentProcessID(_ webView: WKWebView) -> pid_t? {
    diagnosticProcessID(webView, selectorName: "_webProcessIdentifier")
}

@MainActor
func benchmarkWebKitHelperProcessIDsByRole(_ webView: WKWebView) -> [String: pid_t] {
    var result = [String: pid_t]()
    if let pid = benchmarkWebKitWebContentProcessID(webView) {
        result["webcontent"] = pid
    }
    if let pid = diagnosticProcessID(
        webView,
        selectorName: "_networkProcessIdentifier"
    ) {
        result["network"] = pid
    }
    if let pid = diagnosticProcessID(
        webView,
        selectorName: "_gpuProcessIdentifier"
    ) {
        result["gpu"] = pid
    }
    return result
}

@MainActor
func benchmarkWebKitHelperProcessIDs(_ webView: WKWebView) -> [pid_t] {
    Set(benchmarkWebKitHelperProcessIDsByRole(webView).values).sorted()
}

func benchmarkSignalLiveProcessGroup(
    _ processID: pid_t,
    expectedBirthUnixNanoseconds: UInt64,
    signal: Int32
) -> Bool {
    guard processID > 0,
          benchmarkProcessMatchesIdentity(
            pid: processID,
            birthUnixNanoseconds: expectedBirthUnixNanoseconds
          ),
          Darwin.kill(processID, 0) == 0,
          getpgid(processID) == processID else {
        return false
    }
    return Darwin.kill(-processID, signal) == 0
}
