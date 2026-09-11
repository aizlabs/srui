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
    let roles: FixtureRoles
    let nodes: [FixtureNode]

    enum CodingKeys: String, CodingKey {
        case name, roles, nodes
        case firstPaintNodeCount = "first_paint_node_count"
    }
}

struct FixtureRoles: Decodable {
    let surface: UInt64
    let progress: UInt64
    let fileTree: UInt64
    let textEditor: UInt64
    let primaryAction: UInt64

    enum CodingKeys: String, CodingKey {
        case surface, progress
        case fileTree = "file_tree"
        case textEditor = "text_editor"
        case primaryAction = "primary_action"
    }
}

struct BenchmarkFixtureIndex: Sendable {
    let surface: NodeId
    let progress: NodeId
    let fileTree: NodeId
    let textEditor: NodeId
    let primaryAction: NodeId

    init(fixture: Fixture) throws {
        let nodesByID = Dictionary(grouping: fixture.nodes, by: \.id)
        guard nodesByID.values.allSatisfy({ $0.count == 1 }) else {
            throw BenchmarkFailure.message("benchmark fixture contains duplicate node IDs")
        }
        let roleDefinitions: [(String, UInt64, String)] = [
            ("surface", fixture.roles.surface, "Surface"),
            ("progress", fixture.roles.progress, "Progress"),
            ("file_tree", fixture.roles.fileTree, "Tree"),
            ("text_editor", fixture.roles.textEditor, "TextArea"),
            ("primary_action", fixture.roles.primaryAction, "Button"),
        ]
        for (role, id, expectedType) in roleDefinitions {
            guard let matches = nodesByID[id],
                  matches.count == 1,
                  matches[0].type == expectedType else {
                throw BenchmarkFailure.message(
                    "benchmark fixture role \(role) must identify exactly one \(expectedType) node"
                )
            }
        }
        let roleIDs = roleDefinitions.map(\.1)
        guard Set(roleIDs).count == roleIDs.count else {
            throw BenchmarkFailure.message("benchmark fixture roles must identify distinct nodes")
        }
        surface = NodeId(fixture.roles.surface)
        progress = NodeId(fixture.roles.progress)
        fileTree = NodeId(fixture.roles.fileTree)
        textEditor = NodeId(fixture.roles.textEditor)
        primaryAction = NodeId(fixture.roles.primaryAction)
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
    let contractSchemaVersion: Int
    let contractSHA256: String
    let artifacts: Artifacts
    let sections: [Section]

    init(artifacts: Artifacts, sections: [Section]) {
        contractSchemaVersion = BenchmarkMetricContract.schemaVersion
        contractSHA256 = BenchmarkMetricContract.sha256
        self.artifacts = artifacts
        self.sections = sections
    }

    enum CodingKeys: String, CodingKey {
        case contractSchemaVersion = "contract_schema_version"
        case contractSHA256 = "contract_sha256"
        case artifacts, sections
    }
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
    let supervisedParent: Bool

    var requiresCompositedPresentation: Bool {
        profile == "full"
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
                  let birthIndex = values.firstIndex(of: "--driver-birth-unix-ns"),
                  values.indices.contains(birthIndex + 1),
                  let parsedBirth = UInt64(values[birthIndex + 1]),
                  parsedBirth > 0 else {
                throw BenchmarkFailure.message("missing explicit driver identity")
            }
            driverPID = parsedPID
            driverBirthUnixNanoseconds = parsedBirth
        } else {
            guard supervisedParent == false else {
                throw BenchmarkFailure.message("--supervised-parent requires --candidate")
            }
            driverPID = getpid()
            driverBirthUnixNanoseconds = nil
        }
    }
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

enum BenchmarkStatisticsError: Error, Equatable, CustomStringConvertible {
    case noSamples
    case invalidFraction
    case nonfiniteSamples

    var description: String {
        switch self {
        case .noSamples:
            "benchmark percentile contract requires at least one sample"
        case .invalidFraction:
            "benchmark percentile fraction must be finite and between zero and one"
        case .nonfiniteSamples:
            "benchmark percentile samples must all be finite"
        }
    }
}

func checkedPercentile(
    _ values: [Double],
    _ fraction: Double
) -> Result<Double, BenchmarkStatisticsError> {
    guard values.isEmpty == false else {
        return .failure(.noSamples)
    }
    guard fraction.isFinite && (0.0...1.0).contains(fraction) else {
        return .failure(.invalidFraction)
    }
    guard values.allSatisfy(\.isFinite) else {
        return .failure(.nonfiniteSamples)
    }
    let ordered = values.sorted()
    let rank = Double(ordered.count - 1) * fraction
    // Nearest-rank-index with an explicit half-up tie rule. Do not use the
    // language-default rounding mode: cross-driver reports must agree.
    let index = min(
        ordered.count - 1,
        max(0, Int(floor(rank + 0.5)))
    )
    return .success(ordered[index])
}

func percentile(_ values: [Double], _ fraction: Double) -> Double {
    switch checkedPercentile(values, fraction) {
    case .success(let value):
        value
    case .failure(let error):
        preconditionFailure(error.description)
    }
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
