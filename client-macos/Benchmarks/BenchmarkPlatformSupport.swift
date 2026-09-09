import AppKit
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import WebKit

struct RendererProcessIdentity: Codable {
    let pid: Int32
    let birthUnixNanoseconds: UInt64

    enum CodingKeys: String, CodingKey {
        case pid
        case birthUnixNanoseconds = "birth_unix_ns"
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
    let helperPIDSource: String

    enum CodingKeys: String, CodingKey {
        case candidate
        case driverPID = "driver_pid"
        case hostPID = "host_pid"
        case helperPIDs = "helper_pids"
        case processIdentities = "process_identities"
        case startedUnixNanoseconds = "started_unix_ns"
        case endedUnixNanoseconds = "ended_unix_ns"
        case helperPIDSource = "helper_pid_source"
    }
}

struct ProcessResourceSample {
    let cpuMilliseconds: Double
    let physicalFootprintMiB: Double
    let lifetimePeakPhysicalFootprintMiB: Double
    let measuredPIDCount: Int
}

struct OnScreenPaintObservation {
    let crossedDisplayRefresh: Bool
    let captureAuthorization: Bool
    let pixelCaptureVerified: Bool
}

func benchmarkProcessResourceSample(pids: [pid_t]) -> ProcessResourceSample {
    var cpuNanoseconds: UInt64 = 0
    var footprintBytes: UInt64 = 0
    var lifetimePeakFootprintBytes: UInt64 = 0
    var measured = 0
    for pid in Set(pids).sorted() where pid > 0 {
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
        lifetimePeakFootprintBytes &+= usage.ri_lifetime_max_phys_footprint
    }
    return ProcessResourceSample(
        cpuMilliseconds: Double(cpuNanoseconds) / 1_000_000.0,
        physicalFootprintMiB: Double(footprintBytes) / 1_048_576.0,
        lifetimePeakPhysicalFootprintMiB:
            Double(lifetimePeakFootprintBytes) / 1_048_576.0,
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
        birthUnixNanoseconds: birthUnixNanoseconds
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

private func windowServerReportsOnscreen(_ windowID: CGWindowID) -> Bool {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionIncludingWindow, .excludeDesktopElements],
        windowID
    ) as? [[String: Any]] else {
        return false
    }
    return entries.contains { entry in
        (entry[kCGWindowNumber as String] as? NSNumber)?.uint32Value == windowID
            && (entry[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue == true
    }
}

@MainActor
func benchmarkAwaitFramebufferAdvance(
    screen: NSScreen,
    after baseline: TimeInterval
) throws {
    let deadline = Date().addingTimeInterval(2)
    while screen.lastDisplayUpdateTimestamp <= baseline, Date() < deadline {
        _ = RunLoop.current.run(
            mode: .default,
            before: min(deadline, Date().addingTimeInterval(0.005))
        )
    }
    guard screen.lastDisplayUpdateTimestamp > baseline else {
        throw BenchmarkFailure.message(
            "display framebuffer timestamp did not advance after window submission"
        )
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
func benchmarkObserveOnScreenPaint(_ window: NSWindow) async throws -> OnScreenPaintObservation {
    guard let screen = window.screen ?? NSScreen.main else {
        throw BenchmarkFailure.message("benchmark window has no display")
    }
    let framebufferBeforeSubmission = screen.lastDisplayUpdateTimestamp
    window.makeKeyAndOrderFront(nil)
    window.contentView?.layoutSubtreeIfNeeded()
    window.contentView?.needsDisplay = true
    window.displayIfNeeded()
    CATransaction.flush()

    let visibilityDeadline = Date().addingTimeInterval(2)
    while (
        window.isVisible == false
            || window.occlusionState.contains(.visible) == false
            || windowServerReportsOnscreen(CGWindowID(window.windowNumber)) == false
    ) && Date() < visibilityDeadline {
        try await Task.sleep(for: .milliseconds(1))
    }
    let windowID = CGWindowID(window.windowNumber)
    guard window.isVisible,
          window.occlusionState.contains(.visible),
          windowServerReportsOnscreen(windowID) else {
        throw BenchmarkFailure.message(
            "WindowServer did not report benchmark window \(windowID) visible and unoccluded"
        )
    }
    try benchmarkAwaitFramebufferAdvance(
        screen: screen,
        after: framebufferBeforeSubmission
    )

    return OnScreenPaintObservation(
        crossedDisplayRefresh: true,
        captureAuthorization: CGPreflightScreenCaptureAccess(),
        pixelCaptureVerified: false
    )
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
func benchmarkWebKitHelperProcessIDs(_ webView: WKWebView) -> [pid_t] {
    Set([
        benchmarkWebKitWebContentProcessID(webView),
        diagnosticProcessID(webView, selectorName: "_networkProcessIdentifier"),
        diagnosticProcessID(webView, selectorName: "_gpuProcessIdentifier"),
    ].compactMap { $0 }).sorted()
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
