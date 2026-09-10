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

struct Fixture: Decodable {
    let name: String
    let firstPaintNodeCount: Int
    let nodes: [FixtureNode]

    enum CodingKeys: String, CodingKey {
        case name
        case firstPaintNodeCount = "first_paint_node_count"
        case nodes
    }
}

struct FixtureNode: Decodable {
    let id: UInt64
    let type: String
    let parent: UInt64?
    let properties: [String: FixtureValue]?
}

enum FixtureValue: Codable, Equatable {
    case string(String)
    case bool(Bool)
    case number(Double)
    case list([FixtureValue])

    init(from decoder: any Swift.Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode([FixtureValue].self) {
            self = .list(value)
        } else {
            self = .string(try container.decode(String.self))
        }
    }

    func encode(to encoder: any Swift.Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value): try container.encode(value)
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .list(let value): try container.encode(value)
        }
    }

    var semanticValue: SemanticModel.Value {
        switch self {
        case .string(let value): .string(value)
        case .bool(let value): .bool(value)
        case .number(let value): .float64(value)
        case .list(let value): .list(value.map(\.semanticValue))
        }
    }

    var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    var numberValue: Double? {
        if case .number(let value) = self { return value }
        return nil
    }

    var stringListValue: [String]? {
        guard case .list(let values) = self else { return nil }
        let strings = values.compactMap(\.stringValue)
        return strings.count == values.count ? strings : nil
    }
}

struct Output: Encodable {
    let artifacts: Artifacts
    let sections: [Section]
}

struct Artifacts: Encodable {
    let canonicalTransactionSHA256: String
    let canonicalTransactionBytes: Int
    let rendererProcessAttribution: [RendererProcessAttribution]

    enum CodingKeys: String, CodingKey {
        case canonicalTransactionSHA256 = "canonical_transaction_sha256"
        case canonicalTransactionBytes = "canonical_transaction_bytes"
        case rendererProcessAttribution = "renderer_process_attribution"
    }
}

struct Section: Encodable {
    let id: String
    let name: String
    var sampleCounts: [String: Int] = [:]
    var metrics: [Metric]
    var assertions: [Assertion]
    var notes: [String]

    enum CodingKeys: String, CodingKey {
        case id, name, metrics, assertions, notes
        case sampleCounts = "sample_counts"
    }
}

struct Metric: Encodable {
    let id: String
    let name: String
    let value: Double
    let unit: String
    let statistic: String
    var target: Double?
    var targetDirection: String?

    enum CodingKeys: String, CodingKey {
        case id, name, value, unit, statistic, target
        case targetDirection = "target_direction"
    }
}

struct Assertion: Encodable {
    let id: String
    let name: String
    let passed: Bool
    let detail: String

    init(id: String? = nil, name: String, passed: Bool, detail: String) {
        self.id = id ?? semanticIdentifier(name)
        self.name = name
        self.passed = passed
        self.detail = detail
    }
}

struct Arguments {
    let fixture: URL
    let output: URL
    let profile: String
    let candidate: String?
    let onlySection: String?
    let driverPID: Int32
    let driverBirthUnixNanoseconds: UInt64?
    let allocationControlDirectory: URL?
    let allocationTargetRole: String?
    let supervisedParent: Bool

    var isAllocationCaptureCandidate: Bool {
        allocationControlDirectory != nil
    }

    var requiresCompositedPresentation: Bool {
        profile == "full" && !isAllocationCaptureCandidate
    }

    init() throws {
        let values = CommandLine.arguments
        func value(after name: String) throws -> String {
            guard let index = values.firstIndex(of: name), values.indices.contains(index + 1) else {
                throw BenchmarkFailure.message("missing \(name)")
            }
            return values[index + 1]
        }
        fixture = URL(fileURLWithPath: try value(after: "--fixture"))
        output = URL(fileURLWithPath: try value(after: "--output"))
        profile = try value(after: "--profile")
        if let index = values.firstIndex(of: "--candidate"), values.indices.contains(index + 1) {
            candidate = values[index + 1]
        } else {
            candidate = nil
        }
        if let index = values.firstIndex(of: "--only-section"), values.indices.contains(index + 1) {
            onlySection = values[index + 1]
        } else {
            onlySection = nil
        }
        let supervisedParent = values.contains("--supervised-parent")
        self.supervisedParent = supervisedParent
        if candidate != nil, supervisedParent {
            guard values.contains("--driver-pid") == false,
                  values.contains("--driver-birth-unix-ns") == false else {
                throw BenchmarkFailure.message(
                    "--supervised-parent cannot be combined with explicit driver identity"
                )
            }
            let parentPID = getppid()
            guard let parentIdentity = benchmarkProcessIdentity(pid: parentPID) else {
                throw BenchmarkFailure.message(
                    "supervised parent birth identity was unavailable"
                )
            }
            driverPID = parentPID
            driverBirthUnixNanoseconds = parentIdentity.birthUnixNanoseconds
        } else if candidate != nil {
            guard let pidIndex = values.firstIndex(of: "--driver-pid"),
                  values.indices.contains(pidIndex + 1),
                  let parsedPID = Int32(values[pidIndex + 1]),
                  parsedPID > 0,
                  let birthIndex = values.firstIndex(
                      of: "--driver-birth-unix-ns"
                  ),
                  values.indices.contains(birthIndex + 1),
                  let parsedBirth = UInt64(values[birthIndex + 1]),
                  parsedBirth > 0 else {
                throw BenchmarkFailure.message("missing explicit driver identity")
            }
            driverPID = parsedPID
            driverBirthUnixNanoseconds = parsedBirth
        } else {
            guard supervisedParent == false else {
                throw BenchmarkFailure.message(
                    "--supervised-parent requires --candidate"
                )
            }
            driverPID = getpid()
            driverBirthUnixNanoseconds = nil
        }
        let controlIndex = values.firstIndex(of: "--allocation-control-dir")
        let roleIndex = values.firstIndex(of: "--allocation-target-role")
        guard (controlIndex == nil) == (roleIndex == nil) else {
            throw BenchmarkFailure.message(
                "--allocation-control-dir and --allocation-target-role must be used together"
            )
        }
        if let controlIndex, let roleIndex {
            guard candidate != nil,
                  values.indices.contains(controlIndex + 1),
                  values.indices.contains(roleIndex + 1),
                  ["host", "webcontent", "network", "gpu"]
                    .contains(values[roleIndex + 1]) else {
                throw BenchmarkFailure.message("invalid allocation capture control arguments")
            }
            allocationControlDirectory = URL(
                fileURLWithPath: values[controlIndex + 1],
                isDirectory: true
            )
            allocationTargetRole = values[roleIndex + 1]
        } else {
            allocationControlDirectory = nil
            allocationTargetRole = nil
        }
    }
}

struct AllocationCaptureTarget: Encodable {
    let role: String
    let pid: Int32
    let birthUnixNanoseconds: UInt64

    enum CodingKeys: String, CodingKey {
        case role, pid
        case birthUnixNanoseconds = "birth_unix_ns"
    }
}

struct AllocationCaptureRequest: Encodable {
    let schemaVersion = 2
    let candidate: String
    let sampleIndex: Int
    let hostPID: Int32
    let targetRole: String
    let targetPresent: Bool
    let targetPID: Int32?
    let targetBirthUnixNanoseconds: UInt64?
    let availableTargets: [AllocationCaptureTarget]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case candidate
        case sampleIndex = "sample_index"
        case hostPID = "host_pid"
        case targetRole = "target_role"
        case targetPresent = "target_present"
        case targetPID = "target_pid"
        case targetBirthUnixNanoseconds = "target_birth_unix_ns"
        case availableTargets = "available_targets"
    }
}

struct AllocationCaptureDone: Encodable {
    let schemaVersion = 2
    let targetPresent: Bool
    let startedUnixNanoseconds: UInt64
    let endedUnixNanoseconds: UInt64
    let observedAliveThroughUnixNanoseconds: UInt64?

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case targetPresent = "target_present"
        case startedUnixNanoseconds = "started_unix_ns"
        case endedUnixNanoseconds = "ended_unix_ns"
        case observedAliveThroughUnixNanoseconds = "observed_alive_through_unix_ns"
    }
}

struct AllocationCaptureToken {
    let sampleIndex: Int
    let targetRole: String
    let target: AllocationCaptureTarget?
}

@MainActor
struct AllocationCaptureControl {
    let directory: URL
    let targetRole: String

    init(directory: URL, targetRole: String) throws {
        let values = try directory.resourceValues(
            forKeys: [.isDirectoryKey, .isSymbolicLinkKey]
        )
        guard values.isDirectory == true, values.isSymbolicLink != true else {
            throw BenchmarkFailure.message(
                "allocation control directory must be an existing real directory"
            )
        }
        self.directory = directory.standardizedFileURL
        self.targetRole = targetRole
    }

    private func path(_ prefix: String, sampleIndex: Int) -> URL {
        directory.appendingPathComponent(
            "\(prefix)-\(sampleIndex)-\(targetRole)",
            isDirectory: false
        )
    }

    private func publish<T: Encodable>(
        _ value: T,
        to destination: URL
    ) throws {
        let temporary = directory.appendingPathComponent(
            ".\(destination.lastPathComponent).\(UUID().uuidString).tmp",
            isDirectory: false
        )
        let data = try JSONEncoder().encode(value)
        try data.write(to: temporary, options: .withoutOverwriting)
        defer { try? FileManager.default.removeItem(at: temporary) }
        guard Darwin.link(temporary.path, destination.path) == 0 else {
            throw BenchmarkFailure.message(
                "allocation control publication failed for "
                    + "\(destination.lastPathComponent): errno \(errno)"
            )
        }
    }

    private func waitForFile(_ url: URL) async throws {
        let deadline = clock.now + .seconds(60)
        while FileManager.default.fileExists(atPath: url.path) == false {
            guard clock.now < deadline else {
                throw BenchmarkFailure.message(
                    "allocation control timed out waiting for \(url.lastPathComponent)"
                )
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    private func waitForCaptureAcknowledgement(_ url: URL) throws {
        let deadline = clock.now + .seconds(60)
        try url.withUnsafeFileSystemRepresentation { path in
            guard let path else {
                throw BenchmarkFailure.message(
                    "allocation capture acknowledgement path is unavailable"
                )
            }
            while Darwin.access(path, F_OK) != 0 {
                guard errno == ENOENT else {
                    throw BenchmarkFailure.message(
                        "allocation capture acknowledgement check failed for "
                            + "\(url.lastPathComponent): errno \(errno)"
                    )
                }
                guard clock.now < deadline else {
                    throw BenchmarkFailure.message(
                        "allocation control timed out waiting for "
                            + url.lastPathComponent
                    )
                }
                // This is deliberately synchronous and post-measurement. An
                // async Task.sleep loop allocates while xctrace finalizes,
                // making its Statistics and live-list views observe different
                // heap tails.
                Darwin.usleep(10_000)
            }
        }
    }

    func begin(
        candidate: String,
        sampleIndex: Int,
        targetPIDsByRole: [String: Int32]
    ) async throws -> AllocationCaptureToken {
        let allowedRoles = Set(["host", "webcontent", "network", "gpu"])
        guard allowedRoles.contains(targetRole),
              Set(targetPIDsByRole.keys).isSubset(of: allowedRoles),
              targetPIDsByRole["host"] == getpid(),
              candidate == "webkit" || (
                targetRole == "host" && Set(targetPIDsByRole.keys) == ["host"]
              ) else {
            throw BenchmarkFailure.message(
                "allocation target role or available-target map is invalid"
            )
        }

        var availableTargets = [AllocationCaptureTarget]()
        for (role, pid) in targetPIDsByRole.sorted(by: { $0.key < $1.key }) {
            guard let identity = benchmarkProcessIdentity(pid: pid) else {
                throw BenchmarkFailure.message(
                    "allocation target identity is unavailable for role \(role)"
                )
            }
            availableTargets.append(
                AllocationCaptureTarget(
                    role: role,
                    pid: pid,
                    birthUnixNanoseconds: identity.birthUnixNanoseconds
                )
            )
        }
        let target = availableTargets.first { $0.role == targetRole }
        let request = AllocationCaptureRequest(
            candidate: candidate,
            sampleIndex: sampleIndex,
            hostPID: getpid(),
            targetRole: targetRole,
            targetPresent: target != nil,
            targetPID: target?.pid,
            targetBirthUnixNanoseconds: target?.birthUnixNanoseconds,
            availableTargets: availableTargets
        )
        try publish(
            request,
            to: path("request", sampleIndex: sampleIndex).appendingPathExtension("json")
        )
        try await waitForFile(path("go", sampleIndex: sampleIndex))
        guard availableTargets.allSatisfy({
            benchmarkProcessMatchesIdentity(
                pid: $0.pid,
                birthUnixNanoseconds: $0.birthUnixNanoseconds
            )
        }) else {
            throw BenchmarkFailure.message(
                "an allocation target identity changed before measurement"
            )
        }
        return AllocationCaptureToken(
            sampleIndex: sampleIndex,
            targetRole: targetRole,
            target: target
        )
    }

    func finish(
        _ token: AllocationCaptureToken,
        startedUnixNanoseconds: UInt64,
        endedUnixNanoseconds: UInt64
    ) async throws {
        guard startedUnixNanoseconds < endedUnixNanoseconds else {
            throw BenchmarkFailure.message(
                "allocation capture measured interval is empty"
            )
        }
        let observedAliveThroughUnixNanoseconds: UInt64?
        if let target = token.target {
            guard let identity = benchmarkProcessIdentity(pid: target.pid),
                  identity.birthUnixNanoseconds == target.birthUnixNanoseconds,
                  identity.observedAliveThroughUnixNanoseconds
                    >= endedUnixNanoseconds else {
                throw BenchmarkFailure.message(
                    "allocation target identity did not span the measured interval"
                )
            }
            observedAliveThroughUnixNanoseconds =
                identity.observedAliveThroughUnixNanoseconds
        } else {
            observedAliveThroughUnixNanoseconds = nil
        }
        try publish(
            AllocationCaptureDone(
                targetPresent: token.target != nil,
                startedUnixNanoseconds: startedUnixNanoseconds,
                endedUnixNanoseconds: endedUnixNanoseconds,
                observedAliveThroughUnixNanoseconds:
                    observedAliveThroughUnixNanoseconds
            ),
            to: path("done", sampleIndex: token.sampleIndex).appendingPathExtension("json")
        )
        try waitForCaptureAcknowledgement(
            path("captured", sampleIndex: token.sampleIndex)
        )
    }
}

func requiredAllocationPIDs(
    control: AllocationCaptureControl?,
    token: AllocationCaptureToken?
) throws -> [Int32] {
    guard let control else {
        guard token == nil else {
            throw BenchmarkFailure.message(
                "ordinary renderer measurement unexpectedly created an allocation capture token"
            )
        }
        return []
    }
    guard let token, token.targetRole == control.targetRole else {
        throw BenchmarkFailure.message(
            "targeted allocation measurement has no matching capture token"
        )
    }
    if let target = token.target {
        return [target.pid]
    }
    guard control.targetRole == "network" || control.targetRole == "gpu" else {
        throw BenchmarkFailure.message(
            "required allocation target role \(control.targetRole) was absent"
        )
    }
    // The version-2 request/done handshake records target_present=false for
    // optional WebKit roles. An empty requirement is valid only on that path.
    return []
}

enum BenchmarkFailure: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case .message(let message): message
        }
    }
}

let clock = ContinuousClock()

func milliseconds(_ duration: Duration) -> Double {
    let components = duration.components
    return Double(components.seconds) * 1_000.0
        + Double(components.attoseconds) / 1_000_000_000_000_000.0
}

func percentile(_ values: [Double], _ fraction: Double) -> Double {
    guard !values.isEmpty else { return .nan }
    let ordered = values.sorted()
    let index = min(ordered.count - 1, max(0, Int((Double(ordered.count - 1) * fraction).rounded())))
    return ordered[index]
}

func p50(_ values: [Double]) -> Double {
    percentile(values, 0.50)
}

func timed<T>(_ body: () throws -> T) rethrows -> (T, Double) {
    let start = clock.now
    let value = try body()
    return (value, milliseconds(start.duration(to: clock.now)))
}

func semanticIdentifier(_ value: String) -> String {
    value.lowercased().split {
        $0.isLetter == false && $0.isNumber == false
    }.joined(separator: ".")
}

func metric(
    _ name: String,
    _ value: Double,
    _ unit: String = "ms",
    _ statistic: String = "p50",
    target: Double? = nil,
    id: String? = nil
) -> Metric {
    Metric(
        id: id ?? semanticIdentifier(name),
        name: name,
        value: value,
        unit: unit,
        statistic: statistic,
        target: target,
        targetDirection: target == nil ? nil : "max"
    )
}

func digestHex(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

@MainActor
func operations(for fixture: Fixture) throws -> [SemanticModel.Operation] {
    try fixture.nodes.map { node in
        let typeRef = try resolveStandardNodeType(node.type).get()
        let properties = try (node.properties ?? [:])
            .sorted { $0.key < $1.key }
            .map { name, scalar in
                (try resolveStandardProperty(name).get(), scalar.semanticValue)
            }
        return .createNode(
            id: NodeId(node.id),
            nodeType: typeRef,
            parentID: node.parent.map { NodeId($0) },
            properties: properties
        )
    }
}

func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
    var store = SemanticStore()
    try store.apply(operations)
    return store
}

func mallocSample() -> (blocks: Int64, bytes: Int64) {
    var statistics = malloc_statistics_t()
    malloc_zone_statistics(malloc_default_zone(), &statistics)
    return (
        Int64(statistics.blocks_in_use),
        Int64(statistics.size_in_use)
    )
}

func residentPeakMiB() -> Double {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(
        MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size
    )
    let result = withUnsafeMutablePointer(to: &info) { pointer in
        pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    guard result == KERN_SUCCESS else { return -1 }
    return Double(info.resident_size_peak) / 1_048_576.0
}

@MainActor
func renderedBitmapSignature(_ view: NSView) -> Data? {
    view.layoutSubtreeIfNeeded()
    let rect = view.bounds.integral
    guard rect.width > 0, rect.height > 0,
          let representation =
            view.bitmapImageRepForCachingDisplay(in: rect) else {
        return nil
    }
    view.cacheDisplay(in: rect, to: representation)
    guard let bytes = representation.bitmapData else { return nil }
    return Data(
        bytes: bytes,
        count: representation.bytesPerRow * representation.pixelsHigh
    )
}

@MainActor
func rasterize(_ view: NSView) -> Bool {
    view.layoutSubtreeIfNeeded()
    let rect = view.bounds.integral
    guard rect.width > 0, rect.height > 0,
          let representation = view.bitmapImageRepForCachingDisplay(in: rect) else {
        return false
    }
    view.cacheDisplay(in: rect, to: representation)
    return representation.pixelsWide > 0
        && representation.pixelsHigh > 0
        && representation.bitmapData != nil
}

@MainActor
func rasterizeRenderer(_ renderer: AppKitRenderer, showWindows: Bool) -> Bool {
    if showWindows {
        renderer.showWindows()
    }
    var rendered = false
    for surface in renderer.registry.surfaceHandles {
        let view = surface.window?.contentView ?? surface.view
        rendered = rasterize(view) || rendered
    }
    return rendered
}

@MainActor
func closeRenderer(_ renderer: AppKitRenderer) {
    for handle in renderer.registry.surfaceHandles {
        handle.window?.close()
    }
}

@MainActor
func pumpRunLoop(for duration: TimeInterval) {
    let deadline = Date().addingTimeInterval(duration)
    while Date() < deadline {
        _ = RunLoop.current.run(mode: .default, before: min(deadline, Date().addingTimeInterval(0.001)))
    }
}

@MainActor
func pumpWebView(
    _ webView: WKWebView,
    until predicate: () -> Bool,
    timeout: TimeInterval
) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while predicate() == false && Date() < deadline {
        _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.001))
    }
    return predicate()
}
