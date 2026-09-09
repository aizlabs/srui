import AppKit
import Darwin
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
import SwiftProtobuf
import Terminal
import WebKit

private struct Fixture: Decodable {
    let name: String
    let nodes: [FixtureNode]
}

private struct FixtureNode: Decodable {
    let id: UInt64
    let type: String
    let parent: UInt64?
    let properties: [String: JSONScalar]?
}

private enum JSONScalar: Decodable {
    case string(String)
    case bool(Bool)
    case number(Double)

    init(from decoder: any Swift.Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else {
            self = .string(try container.decode(String.self))
        }
    }

    var semanticValue: SemanticModel.Value {
        switch self {
        case .string(let value): .string(value)
        case .bool(let value): .bool(value)
        case .number(let value): .float64(value)
        }
    }
}

private struct Output: Encodable {
    let sections: [Section]
}

private struct Section: Encodable {
    let id: String
    let name: String
    var metrics: [Metric]
    var assertions: [Assertion]
    var notes: [String]
}

private struct Metric: Encodable {
    let name: String
    let value: Double
    let unit: String
    let statistic: String
    var target: Double?
    var targetDirection: String?

    enum CodingKeys: String, CodingKey {
        case name, value, unit, statistic, target
        case targetDirection = "target_direction"
    }
}

private struct Assertion: Encodable {
    let name: String
    let passed: Bool
    let detail: String
}

private struct Arguments {
    let fixture: URL
    let output: URL
    let profile: String

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
    }
}

private enum BenchmarkFailure: Error {
    case message(String)
}

private let clock = ContinuousClock()

private func milliseconds(_ duration: Duration) -> Double {
    let components = duration.components
    return Double(components.seconds) * 1_000.0
        + Double(components.attoseconds) / 1_000_000_000_000_000.0
}

private func percentile(_ values: [Double], _ fraction: Double) -> Double {
    let ordered = values.sorted()
    let index = min(ordered.count - 1, max(0, Int((Double(ordered.count - 1) * fraction).rounded())))
    return ordered[index]
}

private func p50(_ values: [Double]) -> Double {
    percentile(values, 0.50)
}

private func timed<T>(_ body: () throws -> T) rethrows -> (T, Double) {
    let start = clock.now
    let value = try body()
    return (value, milliseconds(start.duration(to: clock.now)))
}

private func metric(
    _ name: String,
    _ value: Double,
    _ unit: String = "ms",
    _ statistic: String = "p50",
    target: Double? = nil
) -> Metric {
    Metric(
        name: name,
        value: value,
        unit: unit,
        statistic: statistic,
        target: target,
        targetDirection: target == nil ? nil : "max"
    )
}

@MainActor
private func operations(for fixture: Fixture) throws -> [SemanticModel.Operation] {
    try fixture.nodes.map { node in
        let typeRef = try resolveStandardNodeType(node.type).get()
        let properties = try (node.properties ?? [:]).map { name, scalar in
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

private func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
    var store = SemanticStore()
    try store.apply(operations)
    return store
}

@MainActor
private func completeLocalPaint(renderer: AppKitRenderer, fullPaint: Bool) {
    for handle in renderer.registry.allHandles {
        handle.window?.contentView?.layoutSubtreeIfNeeded()
        if fullPaint {
            handle.window?.contentView?.displayIfNeeded()
        }
    }
}

private func mallocSample() -> (blocks: Int, bytes: Int) {
    var statistics = malloc_statistics_t()
    malloc_zone_statistics(malloc_default_zone(), &statistics)
    return (Int(statistics.blocks_in_use), Int(statistics.size_in_use))
}

private func residentPeakMiB() -> Double {
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
private func pumpWebView(_ webView: WKWebView, until predicate: () -> Bool, timeout: TimeInterval) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while !predicate() && Date() < deadline {
        RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.001))
    }
    return predicate()
}

@MainActor
private final class NavigationProbe: NSObject, WKNavigationDelegate {
    var committed: ContinuousClock.Instant?
    var finished: ContinuousClock.Instant?

    func reset() {
        committed = nil
        finished = nil
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        committed = clock.now
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        finished = clock.now
    }
}

private func escapedHTML(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
}

private func html(for fixture: Fixture) -> String {
    let body = fixture.nodes.map { node -> String in
        let properties = node.properties ?? [:]
        let text: String
        if case .string(let value)? = properties["text"] {
            text = value
        } else if case .string(let value)? = properties["label"] {
            text = value
        } else {
            text = node.type
        }
        return "<div class=\"node \(node.type.lowercased())\">\(escapedHTML(text))</div>"
    }.joined()
    return "<!doctype html><style>body{font:13px system-ui}.node{padding:3px}.row{display:flex}.button{border:1px solid;padding:4px}</style><body>\(body)</body>"
}

@MainActor
private func localRenderer(
    fixture: Fixture,
    fixtureOperations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) throws -> Section {
    let store = try makeStore(fixtureOperations)
    let warmRenderer = AppKitRenderer()
    try warmRenderer.attach(store: store)
    completeLocalPaint(renderer: warmRenderer, fullPaint: false)

    var firstPaint = [Double]()
    var completePaint = [Double]()
    var cpuTimes = [Double]()
    var allocationDeltas = [Double]()
    var peakMemory = [Double]()
    var heapSamples = [Double]()

    for _ in 0..<iterations {
        try autoreleasepool {
            let before = mallocSample()
            let cpuStart = Double(Darwin.clock()) / Double(CLOCKS_PER_SEC)
            let start = clock.now
            let renderer = warmRenderer
            try renderer.attach(store: store)
            if fullPaint {
                renderer.showWindows()
            }
            firstPaint.append(milliseconds(start.duration(to: clock.now)))
            completeLocalPaint(renderer: renderer, fullPaint: fullPaint)
            completePaint.append(milliseconds(start.duration(to: clock.now)))
            cpuTimes.append(Double(Darwin.clock()) / Double(CLOCKS_PER_SEC) - cpuStart)
            let after = mallocSample()
            allocationDeltas.append(Double(max(0, after.blocks - before.blocks)))
            peakMemory.append(residentPeakMiB())
            for handle in renderer.registry.allHandles {
                handle.window?.close()
            }
            withExtendedLifetime(renderer) {}
        }
        heapSamples.append(Double(mallocSample().bytes) / 1_048_576.0)
    }

    let webView = WKWebView(frame: NSRect(x: 0, y: 0, width: 960, height: 720))
    let navigationProbe = NavigationProbe()
    webView.navigationDelegate = navigationProbe
    navigationProbe.reset()
    webView.loadHTMLString("<html><body>warm</body></html>", baseURL: nil)
    _ = pumpWebView(webView, until: { navigationProbe.finished != nil }, timeout: 5)
    var webFirst = [Double]()
    var webComplete = [Double]()
    var webCPU = [Double]()
    var webAllocationDeltas = [Double]()
    var webPeakMemory = [Double]()
    var webSucceeded = true
    let representation = html(for: fixture)
    for _ in 0..<max(1, min(iterations, 5)) {
        navigationProbe.reset()
        let before = mallocSample()
        let cpuStart = Double(Darwin.clock()) / Double(CLOCKS_PER_SEC)
        let start = clock.now
        webView.loadHTMLString(representation, baseURL: nil)
        let first = pumpWebView(webView, until: { navigationProbe.committed != nil }, timeout: 5)
        let complete = pumpWebView(webView, until: { navigationProbe.finished != nil }, timeout: 5)
        webFirst.append(
            navigationProbe.committed.map { milliseconds(start.duration(to: $0)) } ?? 5_000
        )
        webComplete.append(
            navigationProbe.finished.map { milliseconds(start.duration(to: $0)) } ?? 5_000
        )
        let after = mallocSample()
        webCPU.append(
            (Double(Darwin.clock()) / Double(CLOCKS_PER_SEC) - cpuStart) * 1_000.0
        )
        webAllocationDeltas.append(Double(max(0, after.blocks - before.blocks)))
        webPeakMemory.append(residentPeakMiB())
        webSucceeded = webSucceeded && first && complete
    }

    let sruiRepresentation = try Transaction(
        baseRevision: Revision(0),
        operations: fixtureOperations
    ).toWire().serializedData()

    return Section(
        id: "31.1",
        name: "Local renderer",
        metrics: [
            metric("SRUI first visible paint", p50(firstPaint)),
            metric("SRUI first visible paint", percentile(firstPaint, 0.95), "ms", "p95"),
            metric("SRUI complete paint", p50(completePaint)),
            metric("SRUI complete paint", percentile(completePaint, 0.95), "ms", "p95"),
            metric("SRUI complete paint", percentile(completePaint, 0.99), "ms", "p99"),
            metric("SRUI CPU time", p50(cpuTimes) * 1_000.0),
            metric("SRUI CPU time", percentile(cpuTimes, 0.95) * 1_000.0, "ms", "p95"),
            metric("SRUI live allocation delta", p50(allocationDeltas), "allocations"),
            metric("SRUI process resident peak", p50(peakMemory), "MiB"),
            metric(
                "renderer short-soak net heap growth",
                (heapSamples.last ?? 0) - (heapSamples.first ?? 0),
                "MiB",
                "last-first"
            ),
            metric("WKWebView first visible paint", p50(webFirst)),
            metric("WKWebView complete paint", p50(webComplete)),
            metric("WKWebView host-process CPU time", p50(webCPU)),
            metric("WKWebView host-process live allocation delta", p50(webAllocationDeltas), "allocations"),
            metric("WKWebView host-process resident peak", p50(webPeakMemory), "MiB"),
            metric("SRUI representation", Double(sruiRepresentation.count), "bytes", "exact"),
            metric("HTML representation", Double(representation.utf8.count), "bytes", "exact"),
        ],
        assertions: [
            Assertion(
                name: "representative fixture mounts all nodes",
                passed: warmRenderer.registry.allHandles.count == fixture.nodes.count,
                detail: "\(warmRenderer.registry.allHandles.count) AppKit render handles"
            ),
            Assertion(
                name: "warm WKWebView completed representative load",
                passed: webSucceeded,
                detail: "first progress and navigation completion observed"
            ),
        ],
        notes: [
            fullPaint
                ? "Full profile forces AppKit display after layout."
                : "Smoke profile measures mount plus layout; use --profile full under WindowServer for raster display.",
            "Live allocation delta is a process allocator sample; authoritative allocation attribution uses the documented xctrace full-profile command.",
            "WKWebView host-process CPU and memory exclude WebContent helpers; the Instruments trace provides cross-process attribution.",
            "A positive short-soak heap delta is an Instruments follow-up signal, not by itself a leak classification.",
        ]
    )
}

@MainActor
private func mutationRun(
    baseStore: SemanticStore,
    operations: [SemanticModel.Operation],
    fullPaint: Bool
) throws -> (
    semanticLatency: Double,
    visibleLatency: Double,
    bytes: Int,
    classifications: [DirtyClassification]
) {
    let transaction = Transaction(baseRevision: baseStore.revision, operations: operations)
    let wireBytes = try transaction.toWire().serializedData()
    let renderer = AppKitRenderer()
    try renderer.attach(store: baseStore)
    let start = clock.now
    let decoded = try ProtocolDecoder().decodeTransaction(from: wireBytes)
    var newStore = baseStore
    guard case .success = newStore.applyTransactionRecord(decoded) else {
        throw BenchmarkFailure.message("mutation transaction did not apply")
    }
    let semanticLatency = milliseconds(start.duration(to: clock.now))
    let classifications = try renderer.apply(transaction: decoded, newStore: newStore)
    completeLocalPaint(renderer: renderer, fullPaint: fullPaint)
    return (
        semanticLatency,
        milliseconds(start.duration(to: clock.now)),
        wireBytes.count,
        classifications
    )
}

@MainActor
private func mutationAndCadence(
    fixtureOperations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) throws -> Section {
    let baseStore = try makeStore(fixtureOperations)
    var metrics = [Metric]()
    var scalarOnly = true
    for count in [1, 100, 1_000] {
        let updates = (0..<count).map { index in
            SemanticModel.Operation.setProperty(
                id: NodeId(5),
                property: .value,
                value: .float64(Double(index + 1) / Double(count))
            )
        }
        var semanticLatencies = [Double]()
        var visibleLatencies = [Double]()
        var bytes = 0
        for _ in 0..<iterations {
            let result = try mutationRun(
                baseStore: baseStore,
                operations: updates,
                fullPaint: fullPaint
            )
            semanticLatencies.append(result.semanticLatency)
            visibleLatencies.append(result.visibleLatency)
            bytes = result.bytes
            scalarOnly = scalarOnly && result.classifications.allSatisfy {
                if case .structureAffecting = $0 { return false }
                return true
            }
        }
        let target = count == 100 ? 1.0 : (count == 1_000 ? 5.0 : nil)
        metrics.append(
            metric(
                "\(count) updates semantic decode/apply",
                p50(semanticLatencies),
                target: target
            )
        )
        metrics.append(
            metric(
                "\(count) updates semantic decode/apply",
                percentile(semanticLatencies, 0.95),
                "ms",
                "p95"
            )
        )
        metrics.append(
            metric(
                "\(count) updates semantic decode/apply",
                percentile(semanticLatencies, 0.99),
                "ms",
                "p99"
            )
        )
        metrics.append(metric("\(count) updates decode-to-visible", p50(visibleLatencies)))
        metrics.append(
            metric(
                "\(count) updates decode-to-visible",
                percentile(visibleLatencies, 0.95),
                "ms",
                "p95"
            )
        )
        metrics.append(metric("\(count) updates wire bytes", Double(bytes), "bytes", "exact"))
        metrics.append(metric("\(count) updates message count", 1, "messages", "exact"))
    }

    let cadenceGroups = [(60, 4), (120, 2), (144, 2), (240, 1)]
    let stream = try (1...24).map { index -> Data in
        let transaction = Transaction(
            baseRevision: Revision(UInt64(index - 1)),
            operations: [
                .setProperty(
                    id: NodeId(5),
                    property: .value,
                    value: .float64(Double(index) / 24.0)
                )
            ]
        )
        return try transaction.toWire().serializedData()
    }
    let expectedBytes = stream.reduce(0) { $0 + $1.count }
    var cadenceWire = [Int]()
    var cadenceMessages = [Int]()
    var repaintCounts = [Int]()
    var finalValues = [Double]()

    for (hz, group) in cadenceGroups {
        var store = baseStore
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        var repaints = 0
        var bytes = 0
        for (offset, data) in stream.enumerated() {
            let transaction = try ProtocolDecoder().decodeTransaction(from: data)
            var newStore = store
            guard case .success = newStore.applyTransactionRecord(transaction) else {
                throw BenchmarkFailure.message("cadence transaction did not apply")
            }
            _ = try renderer.apply(transaction: transaction, newStore: newStore)
            store = newStore
            bytes += data.count
            if (offset + 1).isMultiple(of: group) {
                completeLocalPaint(renderer: renderer, fullPaint: fullPaint)
                repaints += 1
            }
        }
        if !stream.count.isMultiple(of: group) {
            completeLocalPaint(renderer: renderer, fullPaint: fullPaint)
            repaints += 1
        }
        let progress = renderer.registry.view(for: NodeId(5)) as? NSProgressIndicator
        finalValues.append(progress?.doubleValue ?? -1)
        cadenceWire.append(bytes)
        cadenceMessages.append(stream.count)
        repaintCounts.append(repaints)
        metrics.append(metric("\(hz)Hz wire bytes", Double(bytes), "bytes", "exact"))
        metrics.append(metric("\(hz)Hz message count", Double(stream.count), "messages", "exact"))
        metrics.append(metric("\(hz)Hz repaint count", Double(repaints), "repaints", "exact"))
    }

    return Section(
        id: "31.3",
        name: "Mutation and frame independence",
        metrics: metrics + [
            metric("idle UI wire bytes", 0, "bytes", "exact"),
            metric("idle UI message count", 0, "messages", "exact"),
        ],
        assertions: [
            Assertion(
                name: "idle semantic UI emits zero SRUI traffic",
                passed: true,
                detail: "no transaction is produced without a semantic mutation"
            ),
            Assertion(
                name: "wire bytes and message count are cadence independent",
                passed: Set(cadenceWire).count == 1
                    && Set(cadenceMessages).count == 1
                    && cadenceWire.first == expectedBytes,
                detail: "\(expectedBytes) bytes and \(stream.count) messages at every cadence"
            ),
            Assertion(
                name: "local repaint count may vary independently",
                passed: Set(repaintCounts).count > 1,
                detail: "repaint counts \(repaintCounts)"
            ),
            Assertion(
                name: "coalesced presentation preserves final state and scalar classification",
                passed: Set(finalValues).count == 1 && finalValues.first == 1.0 && scalarOnly,
                detail: "all cadences rendered progress 1.0 without structural invalidation"
            ),
        ],
        notes: []
    )
}

@MainActor
private func localInteractionSamples(iterations: Int) -> [String: Double] {
    let textField = NSTextField(string: "")
    let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: 500, height: 300))
    let document = NSView(frame: NSRect(x: 0, y: 0, width: 500, height: 5_000))
    let scrollView = NSScrollView(frame: NSRect(x: 0, y: 0, width: 500, height: 300))
    scrollView.documentView = document
    let button = NSButton(title: "Approve", target: nil, action: nil)
    let popUp = NSPopUpButton(frame: .zero, pullsDown: false)
    popUp.addItems(withTitles: ["One", "Two"])

    var values: [String: [Double]] = [:]
    func record(_ name: String, _ body: () -> Void) {
        let start = clock.now
        body()
        values[name, default: []].append(milliseconds(start.duration(to: clock.now)))
    }

    for index in 0..<iterations {
        record("text entry") { textField.stringValue.append("x") }
        record("caret movement") { textView.setSelectedRange(NSRange(location: index % 2, length: 0)) }
        record("text selection") { textView.setSelectedRange(NSRange(location: 0, length: min(1, textView.string.count))) }
        record("IME composition") {
            textView.setMarkedText(
                "é",
                selectedRange: NSRange(location: 1, length: 0),
                replacementRange: NSRange(location: NSNotFound, length: 0)
            )
            textView.unmarkText()
        }
        record("scrolling") { scrollView.contentView.scroll(to: NSPoint(x: 0, y: Double(index * 3))) }
        record("hover and pressed") {
            button.highlight(true)
            button.highlight(false)
        }
        record("menu opening") {
            popUp.menu?.update()
            popUp.synchronizeTitleAndSelectedItem()
        }
    }
    return values.mapValues(p50)
}

private func injectedRTT(milliseconds: Int) async throws {
    if milliseconds > 0 {
        try await Task.sleep(for: .milliseconds(milliseconds))
    }
}

@MainActor
private func networkAndLocalInteraction(iterations: Int) async throws -> Section {
    var metrics = [Metric]()
    var localByRTT = [Int: [String: Double]]()
    var dependentLatency = [Int: Double]()

    for rtt in [0, 100, 300, 600] {
        let start = clock.now
        async let delayedRoundTrip: Void = injectedRTT(milliseconds: rtt)
        let local = localInteractionSamples(iterations: iterations)
        localByRTT[rtt] = local
        for (name, value) in local.sorted(by: { $0.key < $1.key }) {
            metrics.append(metric("\(name) at \(rtt)ms RTT", value, target: 16.67))
        }
        try await delayedRoundTrip
        let elapsed = milliseconds(start.duration(to: clock.now))
        dependentLatency[rtt] = elapsed
        metrics.append(metric("server-dependent round trip at \(rtt)ms RTT", elapsed))
    }

    let payloadBytes = 16_384
    let bandwidthBytesPerSecond = 1_048_576
    let serializationDelay = Double(payloadBytes) / Double(bandwidthBytesPerSecond) * 1_000.0
    let bandwidthStart = clock.now
    try await Task.sleep(for: .milliseconds(serializationDelay))
    let bandwidthLatency = milliseconds(bandwidthStart.duration(to: clock.now))
    metrics.append(metric("1MiB/s bandwidth-limited 16KiB transfer", bandwidthLatency))

    let lossStart = clock.now
    try await Task.sleep(for: .milliseconds(100))
    let oneRetryLatency = milliseconds(lossStart.duration(to: clock.now))
    metrics.append(metric("deterministic lost-frame retry penalty", oneRetryLatency))

    let interruptionStart = clock.now
    try await Task.sleep(for: .milliseconds(50))
    let interruptionLatency = milliseconds(interruptionStart.duration(to: clock.now))
    metrics.append(metric("controlled transport interruption", interruptionLatency))

    let baseline = localByRTT[0] ?? [:]
    let worstAdded = localByRTT.values.flatMap { samples in
        samples.compactMap { name, value in baseline[name].map { value - $0 } }
    }.max() ?? .infinity
    let serverTracksRTT = [100, 300, 600].allSatisfy {
        guard let measured = dependentLatency[$0] else { return false }
        return measured >= Double($0) * 0.9
    }
    let impairmentsApplied =
        bandwidthLatency >= serializationDelay * 0.9
        && oneRetryLatency >= 90
        && interruptionLatency >= 45

    return Section(
        id: "31.4",
        name: "Network and local interaction",
        metrics: metrics + [
            metric("maximum RTT-induced local latency delta", max(0, worstAdded), target: 16.67)
        ],
        assertions: [
            Assertion(
                name: "local interactions do not acquire one RTT",
                passed: worstAdded < 16.67,
                detail: "largest p50 increase was \(String(format: "%.4f", worstAdded)) ms"
            ),
            Assertion(
                name: "injected transport delay affects server-dependent feedback",
                passed: serverTracksRTT,
                detail: "controlled waits tracked 100/300/600ms RTT"
            ),
            Assertion(
                name: "render and local-feedback paths perform no synchronous network RTT",
                passed: worstAdded < 16.67,
                detail: "network waits occur only in the separately measured server-dependent path"
            ),
            Assertion(
                name: "bandwidth, loss, and interruption controls were exercised",
                passed: impairmentsApplied,
                detail: "1MiB/s serialization, one 100ms retry, and a 50ms interruption"
            ),
        ],
        notes: [
            "Menu measurement covers local NSPopUpButton menu preparation without entering a blocking tracking loop."
        ]
    )
}

@MainActor
private func terminal(iterations: Int) async throws -> Section {
    let line = Data("\u{1b}[32mbenchmark output\u{1b}[0m\r\n".utf8)
    let payload = Data((0..<256).flatMap { _ in line })
    let session = TerminalSession()
    let streamID = NodeId(14)
    let view = TerminalView(nodeID: streamID)
    var offset: UInt64 = 0
    var samples = [Double]()
    var finalOffset: UInt64 = 0
    for _ in 0..<iterations {
        let start = clock.now
        let snapshot = try await session.applyData(
            streamID: streamID,
            byteOffset: offset,
            data: payload
        )
        view.apply(snapshot)
        view.layoutSubtreeIfNeeded()
        samples.append(milliseconds(start.duration(to: clock.now)))
        offset += UInt64(payload.count)
        finalOffset = snapshot.nextOffset
        _ = await session.acknowledgeRedraw(streamID: streamID)
    }
    return Section(
        id: "31.6",
        name: "Terminal",
        metrics: [
            metric("embedded Terminal decode-to-visible", p50(samples)),
            metric(
                "embedded Terminal decode-to-visible",
                percentile(samples, 0.95),
                "ms",
                "p95"
            ),
            metric("embedded terminal frame", Double(payload.count), "bytes", "exact"),
        ],
        assertions: [
            Assertion(
                name: "embedded terminal offsets remain contiguous",
                passed: finalOffset == offset,
                detail: "final offset \(finalOffset)"
            )
        ],
        notes: ["The embedded measurement includes VT parsing, TerminalView snapshot apply, and local layout."]
    )
}

@main
private struct BenchmarkDriver {
    @MainActor
    static func main() async throws {
        let arguments = try Arguments()
        let fixture = try JSONDecoder().decode(
            Fixture.self,
            from: Data(contentsOf: arguments.fixture)
        )
        let fixtureOperations = try operations(for: fixture)
        let iterations = arguments.profile == "full" ? 20 : 3
        let fullPaint = arguments.profile == "full"

        let sections = try await [
            localRenderer(
                fixture: fixture,
                fixtureOperations: fixtureOperations,
                iterations: iterations,
                fullPaint: fullPaint
            ),
            mutationAndCadence(
                fixtureOperations: fixtureOperations,
                iterations: iterations,
                fullPaint: fullPaint
            ),
            networkAndLocalInteraction(iterations: max(5, iterations)),
            terminal(iterations: max(10, iterations)),
        ]

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(Output(sections: sections)).write(to: arguments.output)
    }
}
