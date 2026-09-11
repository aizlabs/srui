import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

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

let benchmarkVisibilityMarkerPalette = [
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
    var markerRotation = 0

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
    let normalizedRGBA8Pixels: [UInt8]
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
    let parentPID: pid_t
    let parentBirthUnixNanoseconds: UInt64
    let candidatePID: pid_t
    let candidateBirthUnixNanoseconds: UInt64
    let condition = NSCondition()
    let finished = DispatchSemaphore(value: 0)
    var cancelled = false
    var started = false
    var thread: Thread?

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

    func run() {
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

    func terminateCandidateGroup() -> Never {
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

func withBenchmarkDeadline<T: Sendable>(
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
func benchmarkSessionLockState(
    from dictionary: [String: Any]
) -> Bool? {
    (dictionary["CGSSessionScreenIsLocked"] as? NSNumber)?.boolValue
}

func benchmarkConsoleSessionLockState() -> Bool? {
    guard let dictionary = CGSessionCopyCurrentDictionary()
        as? [String: Any] else {
        return nil
    }
    return benchmarkSessionLockState(from: dictionary)
}

struct BenchmarkWindowServerEvidence: Equatable, Sendable {
    let windowID: CGWindowID
    let ownerPID: pid_t
    let layer: Int
    let bounds: CGRect
    let alpha: Double
    let displayID: CGDirectDisplayID
}

@MainActor
func benchmarkWindowServerEntryDiagnostic(
    for window: NSWindow
) -> String {
    let windowID = CGWindowID(window.windowNumber)
    guard windowID != 0,
          let entries = CGWindowListCopyWindowInfo(
              [.optionIncludingWindow],
              windowID
          ) as? [[String: Any]],
          entries.isEmpty == false else {
        return "matching_entry=absent"
    }
    return entries.map { entry in
        let number = (entry[kCGWindowNumber as String] as? NSNumber)?
            .uint32Value
        let ownerPID = (entry[kCGWindowOwnerPID as String] as? NSNumber)?
            .int32Value
        let onScreen = (entry[kCGWindowIsOnscreen as String] as? NSNumber)?
            .boolValue
        let layer = (entry[kCGWindowLayer as String] as? NSNumber)?
            .intValue
        let alpha = (entry[kCGWindowAlpha as String] as? NSNumber)?
            .doubleValue
        let bounds = entry[kCGWindowBounds as String] as? NSDictionary
        return "entry(number=\(String(describing: number)), "
            + "owner_pid=\(String(describing: ownerPID)), "
            + "on_screen=\(String(describing: onScreen)), "
            + "layer=\(String(describing: layer)), "
            + "alpha=\(String(describing: alpha)), "
            + "bounds=\(String(describing: bounds)))"
    }.joined(separator: ", ")
}

@MainActor
func exactWindowServerEvidence(
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
          // Membership in this .optionOnScreenOnly result is the on-screen
          // proof. WindowServer may omit the redundant kCGWindowIsOnscreen
          // dictionary key even for entries returned by that filtered query.
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

func benchmarkIsolatedHostWindowLevel() throws -> NSWindow.Level {
    let dockLevel = Int(CGWindowLevelForKey(.dockWindow))
    let statusLevel = Int(CGWindowLevelForKey(.statusWindow))
    let popUpMenuLevel = Int(CGWindowLevelForKey(.popUpMenuWindow))
    let screenSaverLevel = Int(CGWindowLevelForKey(.screenSaverWindow))
    guard dockLevel < statusLevel,
          statusLevel < popUpMenuLevel,
          popUpMenuLevel < screenSaverLevel else {
        throw BenchmarkFailure.message(
            "WindowServer levels cannot isolate benchmark hosts between Dock "
                + "and pop-up menus: dock=\(dockLevel), "
                + "status=\(statusLevel), popup=\(popUpMenuLevel), "
                + "screen-saver=\(screenSaverLevel)"
        )
    }
    return NSWindow.Level(rawValue: statusLevel)
}
enum BenchmarkPointerParkingSide {
    case left
    case right
}

func benchmarkPostPointerMoved(at location: CGPoint) -> Bool {
    guard let event = CGEvent(
        mouseEventSource: nil,
        mouseType: .mouseMoved,
        mouseCursorPosition: location,
        mouseButton: .left
    ) else {
        return false
    }
    // CGWarpMouseCursorPosition intentionally emits no mouse event. Notify the
    // active session so the previously hovered application can retract stale
    // tooltips before z-order evidence is captured.
    event.post(tap: .cgSessionEventTap)
    return true
}

@MainActor
func benchmarkParkPointerOutsideMeasurementROI(
    on screen: NSScreen,
    side: BenchmarkPointerParkingSide
) throws -> CGPoint {
    guard let screenNumber = screen.deviceDescription[
        NSDeviceDescriptionKey("NSScreenNumber")
    ] as? NSNumber,
          let currentEvent = CGEvent(source: nil) else {
        throw BenchmarkFailure.message(
            "benchmark could not resolve the pointer or display identity"
        )
    }
    let displayID = CGDirectDisplayID(screenNumber.uint32Value)
    let displayBounds = CGDisplayBounds(displayID)
    // Stay clear of the measurement ROI without touching macOS activation
    // edges used by the auto-hidden Dock, menu bar, and hot corners.
    let horizontalInset: CGFloat = 160
    let parkedX = switch side {
    case .left:
        displayBounds.minX + horizontalInset
    case .right:
        displayBounds.maxX - horizontalInset
    }
    let parkedLocation = CGPoint(x: parkedX, y: displayBounds.midY)
    guard CGWarpMouseCursorPosition(parkedLocation) == .success else {
        throw BenchmarkFailure.message(
            "benchmark could not park the pointer outside the measurement ROI"
        )
    }
    guard benchmarkPostPointerMoved(at: parkedLocation) else {
        _ = CGWarpMouseCursorPosition(currentEvent.location)
        throw BenchmarkFailure.message(
            "benchmark could not notify the session of the parked pointer"
        )
    }

    let expectedAppKitX = switch side {
    case .left:
        screen.frame.minX + horizontalInset
    case .right:
        screen.frame.maxX - horizontalInset
    }
    let deadline = Date().addingTimeInterval(1)
    while abs(NSEvent.mouseLocation.x - expectedAppKitX) > 8,
          Date() < deadline {
        pumpRunLoop(for: 0.01)
    }
    guard abs(NSEvent.mouseLocation.x - expectedAppKitX) <= 8 else {
        _ = CGWarpMouseCursorPosition(currentEvent.location)
        throw BenchmarkFailure.message(
            "WindowServer did not move the pointer to the prepared interior position"
        )
    }
    return currentEvent.location
}

func benchmarkRestorePointer(_ location: CGPoint) {
    _ = CGWarpMouseCursorPosition(location)
    _ = benchmarkPostPointerMoved(at: location)
}

func windowServerNonzeroAlphaIntersectionAbove(
    _ evidence: BenchmarkWindowServerEvidence
) -> String? {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        return "WindowServer window list was unavailable"
    }
    let targetIndices = entries.indices.filter { index in
        (entries[index][kCGWindowNumber as String] as? NSNumber)?.uint32Value
            == evidence.windowID
    }
    guard targetIndices.count == 1, let targetIndex = targetIndices.first else {
        return "target window did not have exactly one z-order entry"
    }

    for index in entries.indices where index < targetIndex {
        let entry = entries[index]
        let ownerName =
            entry[kCGWindowOwnerName as String] as? String ?? "<unknown>"
        // The source list was requested with .optionOnScreenOnly;
        // do not require its optional redundant metadata key.
        guard let windowNumber = (
                  entry[kCGWindowNumber as String] as? NSNumber
              )?.uint32Value,
              let ownerPID = (
                  entry[kCGWindowOwnerPID as String] as? NSNumber
              )?.int32Value,
              let layer = (
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
            return "ahead z-order entry \(index) owned by \(ownerName) "
                + "omitted required identity or geometry"
        }
        guard windowNumber != evidence.windowID else {
            return "target window appeared twice in z-order"
        }
        let intersection = bounds.intersection(evidence.bounds)
        if alpha > 0,
           intersection.isNull == false,
           intersection.width > 0,
           intersection.height > 0 {
            return "owner=\(ownerName) pid=\(ownerPID) "
                + "window=\(windowNumber) layer=\(layer) alpha=\(alpha) "
                + "bounds=\(bounds) intersection=\(intersection)"
        }
    }
    return nil
}

@MainActor
func benchmarkVerifyWindowServerIsolation() async throws {
    guard let screen = NSScreen.main else {
        throw BenchmarkFailure.message(
            "window-isolation self-test requires a main display"
        )
    }
    let statusLevel = try benchmarkIsolatedHostWindowLevel()
    let dockLevel = NSWindow.Level(
        rawValue: Int(CGWindowLevelForKey(.dockWindow))
    )
    let popUpMenuLevel = Int(CGWindowLevelForKey(.popUpMenuWindow))
    let syntheticAheadLevel = statusLevel.rawValue + 1
    guard syntheticAheadLevel < popUpMenuLevel else {
        throw BenchmarkFailure.message(
            "window-isolation self-test has no level between status and popup"
        )
    }

    let frame = CGRect(
        x: screen.visibleFrame.midX - 80,
        y: screen.visibleFrame.midY - 60,
        width: 160,
        height: 120
    )
    func makeOpaqueWindow(color: NSColor) -> NSWindow {
        let window = NSWindow(
            contentRect: frame,
            styleMask: [.borderless],
            backing: .buffered,
            defer: false,
            screen: screen
        )
        window.animationBehavior = .none
        window.backgroundColor = color
        window.isOpaque = true
        window.alphaValue = 1
        window.hasShadow = false
        window.ignoresMouseEvents = true
        return window
    }
    let target = makeOpaqueWindow(color: .systemBlue)
    let syntheticOccluder = makeOpaqueWindow(color: .systemRed)
    defer {
        syntheticOccluder.orderOut(nil)
        target.orderOut(nil)
    }

    func awaitEvidence(
        for window: NSWindow,
        expectedLevel: NSWindow.Level
    ) async throws -> BenchmarkWindowServerEvidence {
        let deadline = Date().addingTimeInterval(2)
        while Date() < deadline {
            if let evidence = exactWindowServerEvidence(
                for: window,
                on: screen
            ), evidence.layer == expectedLevel.rawValue {
                return evidence
            }
            try await Task.sleep(for: .milliseconds(5))
        }
        throw BenchmarkFailure.message(
            "window-isolation self-test did not observe window "
                + "\(window.windowNumber) at layer \(expectedLevel.rawValue)"
        )
    }

    target.level = statusLevel
    target.orderFrontRegardless()
    syntheticOccluder.level = dockLevel
    syntheticOccluder.orderFrontRegardless()
    CATransaction.flush()
    var targetEvidence = try await awaitEvidence(
        for: target,
        expectedLevel: statusLevel
    )
    _ = try await awaitEvidence(
        for: syntheticOccluder,
        expectedLevel: dockLevel
    )
    if let unexpected =
        windowServerNonzeroAlphaIntersectionAbove(targetEvidence)
    {
        throw BenchmarkFailure.message(
            "below-status synthetic window was incorrectly reported ahead: "
                + unexpected
        )
    }

    let aheadLevel = NSWindow.Level(rawValue: syntheticAheadLevel)
    syntheticOccluder.level = aheadLevel
    syntheticOccluder.orderFrontRegardless()
    CATransaction.flush()
    let aheadEvidence = try await awaitEvidence(
        for: syntheticOccluder,
        expectedLevel: aheadLevel
    )
    targetEvidence = try await awaitEvidence(
        for: target,
        expectedLevel: statusLevel
    )
    guard let diagnostic =
        windowServerNonzeroAlphaIntersectionAbove(targetEvidence),
          diagnostic.contains("window=\(aheadEvidence.windowID) "),
          diagnostic.contains("layer=\(aheadEvidence.layer) ") else {
        throw BenchmarkFailure.message(
            "above-status synthetic window was not rejected with its exact "
                + "WindowServer identity"
        )
    }
    benchmarkPhase(
        "window isolation self-test passed: dock=\(dockLevel.rawValue) "
            + "status=\(statusLevel.rawValue) ahead=\(aheadEvidence.layer) "
            + "popup=\(popUpMenuLevel) target=\(targetEvidence.windowID) "
            + "occluder=\(aheadEvidence.windowID)"
    )
}

struct BenchmarkVisibilityMarkerMatch: Sendable {
    let x: Int
    let y: Int
    let step: Int
}

struct BenchmarkCompositedCaptureEvidence: @unchecked Sendable {
    let markerVisible: Bool
    let content: BenchmarkCompositedContentEvidence
    let normalization: BenchmarkCompositedContentNormalization
    let image: CGImage
}
