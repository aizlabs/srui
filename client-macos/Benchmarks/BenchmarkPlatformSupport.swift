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

    enum CodingKeys: String, CodingKey {
        case startedUnixNanoseconds = "started_unix_ns"
        case endedUnixNanoseconds = "ended_unix_ns"
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