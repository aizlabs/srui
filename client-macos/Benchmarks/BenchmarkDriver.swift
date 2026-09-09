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
}

private struct Output: Encodable {
    let artifacts: Artifacts
    let sections: [Section]
}

private struct Artifacts: Encodable {
    let canonicalTransactionSHA256: String
    let canonicalTransactionBytes: Int

    enum CodingKeys: String, CodingKey {
        case canonicalTransactionSHA256 = "canonical_transaction_sha256"
        case canonicalTransactionBytes = "canonical_transaction_bytes"
    }
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
    let candidate: String?
    let onlySection: String?

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
    }
}

private enum BenchmarkFailure: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case .message(let message): message
        }
    }
}

private let clock = ContinuousClock()

private func milliseconds(_ duration: Duration) -> Double {
    let components = duration.components
    return Double(components.seconds) * 1_000.0
        + Double(components.attoseconds) / 1_000_000_000_000_000.0
}

private func percentile(_ values: [Double], _ fraction: Double) -> Double {
    guard !values.isEmpty else { return .nan }
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

private func digestHex(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

@MainActor
private func operations(for fixture: Fixture) throws -> [SemanticModel.Operation] {
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

private func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
    var store = SemanticStore()
    try store.apply(operations)
    return store
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
private func rasterize(_ view: NSView) -> Bool {
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
private func rasterizeRenderer(_ renderer: AppKitRenderer, showWindows: Bool) -> Bool {
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
private func closeRenderer(_ renderer: AppKitRenderer) {
    for handle in renderer.registry.surfaceHandles {
        handle.window?.close()
    }
}

@MainActor
private func pumpRunLoop(for duration: TimeInterval) {
    let deadline = Date().addingTimeInterval(duration)
    while Date() < deadline {
        _ = RunLoop.current.run(mode: .default, before: min(deadline, Date().addingTimeInterval(0.001)))
    }
}

@MainActor
private func pumpWebView(
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

@MainActor
private final class NavigationProbe: NSObject, WKNavigationDelegate {
    var committed: ContinuousClock.Instant?
    var finished: ContinuousClock.Instant?
    var failure: Error?

    func reset() {
        committed = nil
        finished = nil
        failure = nil
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        committed = clock.now
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        finished = clock.now
    }

    func webView(
        _ webView: WKWebView,
        didFail navigation: WKNavigation!,
        withError error: any Error
    ) {
        failure = error
    }

    func webView(
        _ webView: WKWebView,
        didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: any Error
    ) {
        failure = error
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
    let byParent = Dictionary(grouping: fixture.nodes) { $0.parent }
    func children(of node: FixtureNode) -> String {
        (byParent[node.id] ?? []).map(render).joined()
    }
    func attribute(_ name: String, _ value: String?) -> String {
        value.map { " \(name)=\"\(escapedHTML($0))\"" } ?? ""
    }
    func render(_ node: FixtureNode) -> String {
        let properties = node.properties ?? [:]
        let id = " data-srui-id=\"\(node.id)\""
        let label = properties["label"]?.stringValue
        let text = properties["text"]?.stringValue ?? label ?? ""
        let descendants = children(of: node)
        switch node.type {
        case "Surface":
            return "<main\(id) role=\"application\"\(attribute("aria-label", label))>\(descendants)</main>"
        case "Column":
            return "<section\(id) class=\"column\"\(attribute("aria-label", label))>\(descendants)</section>"
        case "Row":
            return "<div\(id) class=\"row\"\(attribute("aria-label", label))>\(descendants)</div>"
        case "Text":
            return "<p\(id)>\(escapedHTML(text))\(descendants)</p>"
        case "RichText":
            return "<pre\(id)>\(escapedHTML(text))\(descendants)</pre>"
        case "Progress":
            let value = properties["value"]?.numberValue ?? 0
            return "<progress\(id) max=\"1\" value=\"\(value)\"\(attribute("aria-valuetext", properties["value_description"]?.stringValue))></progress>\(descendants)"
        case "Tree":
            return "<nav\(id) role=\"tree\"\(attribute("aria-label", label))>\(descendants)</nav>"
        case "TextArea":
            let value = properties["value"]?.stringValue ?? ""
            return "<textarea\(id)\(attribute("aria-label", label))>\(escapedHTML(value))</textarea>\(descendants)"
        case "Button":
            let disabled = properties["enabled"]?.boolValue == false ? " disabled" : ""
            return "<button\(id)\(disabled)>\(escapedHTML(label ?? "Button"))</button>\(descendants)"
        case "Separator":
            return "<hr\(id)>\(descendants)"
        default:
            return "<div\(id) role=\"group\"\(attribute("aria-label", label))>\(escapedHTML(text))\(descendants)</div>"
        }
    }
    let body = (byParent[nil] ?? []).map(render).joined()
    return """
    <!doctype html><html><head><meta charset="utf-8"><style>
    html,body{margin:0;padding:0}body{font:13px -apple-system,system-ui;padding:20px}
    main,.column{display:flex;flex-direction:column;gap:10px}.row{display:flex;gap:10px}
    section,.row{min-width:0}p,pre{margin:0;white-space:pre-wrap}
    textarea{min-width:360px;min-height:90px}button{padding:5px 12px}
    progress{width:180px}nav[role=tree]{min-width:120px;min-height:80px}
    </style></head><body>\(body)</body></html>
    """
}

@MainActor
private final class AnimationFrameProbe: NSObject, WKScriptMessageHandler {
    private weak var controller: WKUserContentController?
    private let name: String
    private var continuation: CheckedContinuation<Void, any Error>?

    init(
        controller: WKUserContentController,
        name: String,
        continuation: CheckedContinuation<Void, any Error>
    ) {
        self.controller = controller
        self.name = name
        self.continuation = continuation
    }

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        finish()
    }

    func fail(_ error: any Error) {
        finish(error)
    }

    private func finish(_ error: (any Error)? = nil) {
        guard let continuation else { return }
        self.continuation = nil
        controller?.removeScriptMessageHandler(forName: name)
        if let error {
            continuation.resume(throwing: error)
        } else {
            continuation.resume()
        }
    }
}

@MainActor
private func nextAnimationFrame(in webView: WKWebView) async throws {
    let controller = webView.configuration.userContentController
    let name = "sruiFrame" + UUID().uuidString.replacingOccurrences(of: "-", with: "")
    try await withCheckedThrowingContinuation {
        (continuation: CheckedContinuation<Void, any Error>) in
        let probe = AnimationFrameProbe(
            controller: controller,
            name: name,
            continuation: continuation
        )
        controller.add(probe, name: name)
        webView.evaluateJavaScript(
            "requestAnimationFrame(() => window.webkit.messageHandlers.\(name).postMessage(true));"
        ) { _, error in
            if let error {
                probe.fail(error)
            }
        }
    }
}

@MainActor
private func snapshot(_ webView: WKWebView) async throws -> NSImage {
    let configuration = WKSnapshotConfiguration()
    configuration.rect = webView.bounds
    return try await withCheckedThrowingContinuation { continuation in
        webView.takeSnapshot(with: configuration) { image, error in
            if let error {
                continuation.resume(throwing: error)
            } else if let image {
                continuation.resume(returning: image)
            } else {
                continuation.resume(throwing: BenchmarkFailure.message("WKWebView snapshot returned no image"))
            }
        }
    }
}

@MainActor
private func javascriptNumber(_ script: String, in webView: WKWebView) async throws -> Int {
    try await withCheckedThrowingContinuation { continuation in
        webView.evaluateJavaScript(script) { value, error in
            if let error {
                continuation.resume(throwing: error)
            } else if let number = value as? NSNumber {
                continuation.resume(returning: number.intValue)
            } else {
                continuation.resume(throwing: BenchmarkFailure.message("JavaScript did not return a number"))
            }
        }
    }
}

private struct RendererCandidateResult: Codable {
    let candidate: String
    let firstPaint: [Double]
    let completePaint: [Double]
    let cpuTime: [Double]
    let liveAllocationDelta: [Double]
    let processResidentPeak: [Double]
    let heapGrowthMiB: Double
    let renderedNodeCount: Int
    let pixelCompletions: Int
    let representationBytes: Int
    let succeeded: Bool
}

@MainActor
private func runSRUICandidate(
    fixture: Fixture,
    operations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) throws -> RendererCandidateResult {
    let store = try makeStore(operations)
    let warm = AppKitRenderer()
    try warm.attach(store: store)
    _ = rasterizeRenderer(warm, showWindows: fullPaint)
    closeRenderer(warm)

    let heapStart = mallocSample().bytes
    var first = [Double]()
    var complete = [Double]()
    var cpu = [Double]()
    var allocations = [Double]()
    var peaks = [Double]()
    var renderedNodeCount = 0
    var pixelCompletions = 0

    for _ in 0..<iterations {
        try autoreleasepool {
            let before = mallocSample()
            let cpuStart = Double(Darwin.clock()) / Double(CLOCKS_PER_SEC)
            let started = clock.now
            let renderer = AppKitRenderer()
            try renderer.attach(store: store)
            for surface in renderer.registry.surfaceHandles {
                surface.window?.contentView?.layoutSubtreeIfNeeded()
            }
            let painted = rasterizeRenderer(renderer, showWindows: fullPaint)
            first.append(milliseconds(started.duration(to: clock.now)))
            pumpRunLoop(for: 0.001)
            let completePainted = rasterizeRenderer(renderer, showWindows: false)
            complete.append(milliseconds(started.duration(to: clock.now)))
            cpu.append(
                (Double(Darwin.clock()) / Double(CLOCKS_PER_SEC) - cpuStart) * 1_000.0
            )
            renderedNodeCount = renderer.registry.allHandles.count
            if painted && completePainted { pixelCompletions += 1 }
            let after = mallocSample()
            allocations.append(Double(max(0, after.blocks - before.blocks)))
            peaks.append(residentPeakMiB())
            closeRenderer(renderer)
            withExtendedLifetime(renderer) {}
        }
    }
    let heapEnd = mallocSample().bytes
    let bytes = try Transaction(baseRevision: Revision(0), operations: operations)
        .toWire()
        .serializedData()
        .count
    return RendererCandidateResult(
        candidate: "srui",
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        liveAllocationDelta: allocations,
        processResidentPeak: peaks,
        heapGrowthMiB: Double(heapEnd - heapStart) / 1_048_576.0,
        renderedNodeCount: renderedNodeCount,
        pixelCompletions: pixelCompletions,
        representationBytes: bytes,
        succeeded: renderedNodeCount == fixture.nodes.count && pixelCompletions == iterations
    )
}

@MainActor
private func loadAndRasterizeWebView(
    _ webView: WKWebView,
    probe: NavigationProbe,
    html: String,
    waitForAnimationFrame: Bool
) async throws -> (first: Double, complete: Double, pixels: Bool, nodeCount: Int) {
    probe.reset()
    let started = clock.now
    webView.loadHTMLString(html, baseURL: nil)
    guard pumpWebView(
        webView,
        until: { probe.finished != nil || probe.failure != nil },
        timeout: 10
    ), probe.failure == nil else {
        throw probe.failure ?? BenchmarkFailure.message("WKWebView navigation timed out")
    }
    if waitForAnimationFrame {
        try await nextAnimationFrame(in: webView)
    } else {
        pumpRunLoop(for: 0.002)
    }
    let firstImage = try await snapshot(webView)
    let first = milliseconds(started.duration(to: clock.now))
    if waitForAnimationFrame {
        try await nextAnimationFrame(in: webView)
    } else {
        pumpRunLoop(for: 0.002)
    }
    let completeImage = try await snapshot(webView)
    let complete = milliseconds(started.duration(to: clock.now))
    let nodeCount = try await javascriptNumber(
        "document.querySelectorAll('[data-srui-id]').length",
        in: webView
    )
    let pixels = firstImage.size.width > 0 && firstImage.size.height > 0
        && completeImage.size.width > 0 && completeImage.size.height > 0
    return (first, complete, pixels, nodeCount)
}

@MainActor
private func runWebCandidate(
    fixture: Fixture,
    iterations: Int,
    fullPaint: Bool
) async throws -> RendererCandidateResult {
    let representation = html(for: fixture)
    let webView = WKWebView(frame: NSRect(x: 0, y: 0, width: 960, height: 720))
    let probe = NavigationProbe()
    webView.navigationDelegate = probe
    let window = NSWindow(
        contentRect: webView.frame,
        styleMask: [.titled, .closable, .resizable],
        backing: .buffered,
        defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentView = webView
    if fullPaint {
        window.makeKeyAndOrderFront(nil)
    }
    _ = try await loadAndRasterizeWebView(
        webView,
        probe: probe,
        html: "<!doctype html><html><body data-srui-id=\"0\">warm</body></html>",
        waitForAnimationFrame: fullPaint
    )

    let heapStart = mallocSample().bytes
    var first = [Double]()
    var complete = [Double]()
    var cpu = [Double]()
    var allocations = [Double]()
    var peaks = [Double]()
    var renderedNodeCount = 0
    var pixelCompletions = 0

    for _ in 0..<max(1, min(iterations, 10)) {
        let before = mallocSample()
        let cpuStart = Double(Darwin.clock()) / Double(CLOCKS_PER_SEC)
        let result = try await loadAndRasterizeWebView(
            webView,
            probe: probe,
            html: representation,
            waitForAnimationFrame: fullPaint
        )
        first.append(result.first)
        complete.append(result.complete)
        cpu.append(
            (Double(Darwin.clock()) / Double(CLOCKS_PER_SEC) - cpuStart) * 1_000.0
        )
        renderedNodeCount = result.nodeCount
        if result.pixels { pixelCompletions += 1 }
        let after = mallocSample()
        allocations.append(Double(max(0, after.blocks - before.blocks)))
        peaks.append(residentPeakMiB())
    }
    let heapEnd = mallocSample().bytes
    window.close()
    let measuredIterations = max(1, min(iterations, 10))
    return RendererCandidateResult(
        candidate: "webkit",
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        liveAllocationDelta: allocations,
        processResidentPeak: peaks,
        heapGrowthMiB: Double(heapEnd - heapStart) / 1_048_576.0,
        renderedNodeCount: renderedNodeCount,
        pixelCompletions: pixelCompletions,
        representationBytes: representation.utf8.count,
        succeeded: renderedNodeCount == fixture.nodes.count
            && pixelCompletions == measuredIterations
    )
}

@MainActor
private func runCandidateSubprocess(
    name: String,
    fixture: URL,
    profile: String
) throws -> RendererCandidateResult {
    let temporary = FileManager.default.temporaryDirectory
        .appendingPathComponent("srui-benchmark-\(name)-\(UUID().uuidString).json")
    defer { try? FileManager.default.removeItem(at: temporary) }

    let executablePath = CommandLine.arguments[0]
    let executable = URL(
        fileURLWithPath: executablePath,
        relativeTo: URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    ).standardizedFileURL
    let process = Process()
    process.executableURL = executable
    process.arguments = [
        "--fixture", fixture.path,
        "--output", temporary.path,
        "--profile", profile,
        "--candidate", name,
    ]
    try process.run()
    let deadline = Date().addingTimeInterval(profile == "full" ? 240 : 60)
    while process.isRunning && Date() < deadline {
        _ = RunLoop.current.run(
            mode: .default,
            before: min(deadline, Date().addingTimeInterval(0.01))
        )
    }
    if process.isRunning {
        process.terminate()
        let terminationDeadline = Date().addingTimeInterval(2)
        while process.isRunning && Date() < terminationDeadline {
            _ = RunLoop.current.run(
                mode: .default,
                before: min(terminationDeadline, Date().addingTimeInterval(0.01))
            )
        }
        if process.isRunning {
            _ = Darwin.kill(process.processIdentifier, SIGKILL)
            let killDeadline = Date().addingTimeInterval(2)
            while process.isRunning && Date() < killDeadline {
                _ = RunLoop.current.run(
                    mode: .default,
                    before: min(killDeadline, Date().addingTimeInterval(0.01))
                )
            }
        }
        throw BenchmarkFailure.message("renderer candidate \(name) timed out")
    }
    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        throw BenchmarkFailure.message(
            "renderer candidate \(name) exited with status \(process.terminationStatus)"
        )
    }
    return try JSONDecoder().decode(
        RendererCandidateResult.self,
        from: Data(contentsOf: temporary)
    )
}

@MainActor
private func localRenderer(
    fixture: Fixture,
    fixtureURL: URL,
    profile: String
) throws -> Section {
    let srui = try runCandidateSubprocess(name: "srui", fixture: fixtureURL, profile: profile)
    let web = try runCandidateSubprocess(name: "webkit", fixture: fixtureURL, profile: profile)

    return Section(
        id: "31.1",
        name: "Local renderer",
        metrics: [
            metric("SRUI first visible paint", p50(srui.firstPaint)),
            metric("SRUI first visible paint", percentile(srui.firstPaint, 0.95), "ms", "p95"),
            metric("SRUI complete paint", p50(srui.completePaint)),
            metric("SRUI complete paint", percentile(srui.completePaint, 0.95), "ms", "p95"),
            metric("SRUI complete paint", percentile(srui.completePaint, 0.99), "ms", "p99"),
            metric("SRUI candidate-process CPU time", p50(srui.cpuTime)),
            metric("SRUI candidate-process CPU time", percentile(srui.cpuTime, 0.95), "ms", "p95"),
            metric("SRUI candidate-process live allocation delta", p50(srui.liveAllocationDelta), "allocations"),
            metric("SRUI candidate-process resident peak", srui.processResidentPeak.max() ?? -1, "MiB", "max"),
            metric("SRUI candidate-process short-soak net heap growth", srui.heapGrowthMiB, "MiB", "last-first"),
            metric("WKWebView first visible paint", p50(web.firstPaint)),
            metric("WKWebView first visible paint", percentile(web.firstPaint, 0.95), "ms", "p95"),
            metric("WKWebView complete paint", p50(web.completePaint)),
            metric("WKWebView complete paint", percentile(web.completePaint, 0.95), "ms", "p95"),
            metric("WKWebView host candidate-process CPU time", p50(web.cpuTime)),
            metric("WKWebView host candidate-process live allocation delta", p50(web.liveAllocationDelta), "allocations"),
            metric("WKWebView host candidate-process resident peak", web.processResidentPeak.max() ?? -1, "MiB", "max"),
            metric("WKWebView host candidate-process short-soak net heap growth", web.heapGrowthMiB, "MiB", "last-first"),
            metric("SRUI representation", Double(srui.representationBytes), "bytes", "exact"),
            metric("HTML representation", Double(web.representationBytes), "bytes", "exact"),
        ],
        assertions: [
            Assertion(
                name: "representative fixture mounts equivalent hierarchical native and HTML nodes",
                passed: srui.renderedNodeCount == fixture.nodes.count
                    && web.renderedNodeCount == fixture.nodes.count,
                detail: "\(srui.renderedNodeCount) AppKit handles and \(web.renderedNodeCount) semantic DOM elements"
            ),
            Assertion(
                name: "first and complete paint are backed by actual raster output",
                passed: srui.succeeded && web.succeeded,
                detail: "\(srui.pixelCompletions) native and \(web.pixelCompletions) WebKit raster completions"
            ),
        ],
        notes: [
            profile == "full"
                ? "Both candidates use ordered windows; completion requires AppKit bitmap rendering or WebKit requestAnimationFrame plus snapshot."
                : "Smoke uses offscreen bitmap/snapshot completion; full orders both windows under WindowServer.",
            "Each candidate runs in a fresh child process, so CPU, allocator, heap-growth, and process peak samples are not contaminated by the other renderer.",
            "Live allocation delta counts retained malloc blocks at paint completion; total allocation-event attribution and WebContent helper costs remain available in the documented Instruments trace.",
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
    classifications: [DirtyClassification],
    rasterized: Bool
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
    let rasterized = rasterizeRenderer(renderer, showWindows: fullPaint)
    let visible = milliseconds(start.duration(to: clock.now))
    closeRenderer(renderer)
    return (semanticLatency, visible, wireBytes.count, classifications, rasterized)
}

private struct CapturedTransportFrame: Sendable {
    let data: Data
    let logicalClass: LogicalChannelClass
}

private struct BenchmarkTransportSnapshot: Sendable {
    let outboundAttempts: Int
    let outboundMessages: Int
    let outboundBytes: Int
    let inboundMessages: Int
    let inboundBytes: Int
    let droppedMessages: Int
    let interruptions: Int
}

private actor BenchmarkTransport: Transport {
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation
    private let oneWayDelayMilliseconds: Double
    private let bytesPerSecond: Int?
    private let dropOutboundOrdinals: Set<Int>
    private let interruptOutboundOrdinals: Set<Int>
    private var closed = false
    private var outboundAttempts = 0
    private var outboundFrames = [CapturedTransportFrame]()
    private var inboundFrames = [Data]()
    private var droppedMessages = 0
    private var interruptions = 0

    init(
        rttMilliseconds: Int = 0,
        bytesPerSecond: Int? = nil,
        dropOutboundOrdinals: Set<Int> = [],
        interruptOutboundOrdinals: Set<Int> = []
    ) {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream(
            bufferingPolicy: .bufferingOldest(4_096)
        )
        self.stream = stream
        self.continuation = continuation
        self.oneWayDelayMilliseconds = Double(rttMilliseconds) / 2.0
        self.bytesPerSecond = bytesPerSecond
        self.dropOutboundOrdinals = dropOutboundOrdinals
        self.interruptOutboundOrdinals = interruptOutboundOrdinals
    }

    deinit {
        continuation.finish()
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        guard closed == false else { throw TransportError.closed }
        outboundAttempts += 1
        let ordinal = outboundAttempts
        try await applyDelay(byteCount: data.count)
        try Task.checkCancellation()
        guard closed == false else { throw TransportError.closed }
        if interruptOutboundOrdinals.contains(ordinal) {
            interruptions += 1
            throw TransportError.closed
        }
        if dropOutboundOrdinals.contains(ordinal) {
            droppedMessages += 1
            return
        }
        outboundFrames.append(CapturedTransportFrame(data: data, logicalClass: logicalClass))
    }

    func injectFromServer(_ data: Data) async throws {
        guard closed == false else { throw TransportError.closed }
        try await applyDelay(byteCount: data.count)
        try Task.checkCancellation()
        guard closed == false else { throw TransportError.closed }
        inboundFrames.append(data)
        continuation.yield(data)
    }

    func close() {
        guard closed == false else { return }
        closed = true
        continuation.finish()
    }

    func framesSent() -> [CapturedTransportFrame] {
        outboundFrames
    }

    func snapshot() -> BenchmarkTransportSnapshot {
        BenchmarkTransportSnapshot(
            outboundAttempts: outboundAttempts,
            outboundMessages: outboundFrames.count,
            outboundBytes: outboundFrames.reduce(0) { $0 + $1.data.count },
            inboundMessages: inboundFrames.count,
            inboundBytes: inboundFrames.reduce(0) { $0 + $1.count },
            droppedMessages: droppedMessages,
            interruptions: interruptions
        )
    }

    private func applyDelay(byteCount: Int) async throws {
        let serializationMilliseconds = bytesPerSecond.map {
            Double(byteCount) / Double($0) * 1_000.0
        } ?? 0
        let total = oneWayDelayMilliseconds + serializationMilliseconds
        if total > 0 {
            try await Task.sleep(for: .milliseconds(total))
        }
    }
}
private func welcomeMessage(sessionID: String) -> SRUIMessage {
    var welcome = SRUIServerWelcome()
    welcome.coreVersion = SRUICoreVersion
    welcome.sessionID = sessionID
    welcome.requiredProfiles = [Profile.standardWidgetsV1.description]
    var message = SRUIMessage()
    message.serverWelcome = welcome
    return message
}

private func resumeOKMessage(sessionID: String, lastProcessedEventSeq: UInt64 = 0) -> SRUIMessage {
    var resume = SRUIServerResumeOk()
    resume.sessionID = sessionID
    resume.replayFromRevision = 1
    resume.lastProcessedEventSeq = lastProcessedEventSeq
    var message = SRUIMessage()
    message.serverResumeOk = resume
    return message
}

private func transactionMessage(_ transaction: Transaction) throws -> SRUIMessage {
    var message = SRUIMessage()
    message.transaction = transaction.toWire()
    return message
}

private func framed(_ message: SRUIMessage) throws -> Data {
    try SRUIFraming.encodeFramed(message)
}

private func acknowledgeMessage(
    _ event: Event,
    outbox: EventOutbox,
    sessionID: String,
    revision: Revision
) -> SRUIMessage {
    var acknowledgement = SRUIServerEventAck()
    acknowledgement.sessionID = sessionID
    acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
    acknowledgement.eventID = event.eventId.bytes
    acknowledgement.lastProcessedEventSeq = event.eventSeq
    acknowledgement.status = .processed
    acknowledgement.revisionAfterEffect = revision.value
    var message = SRUIMessage()
    message.serverEventAck = acknowledgement
    return message
}

private func waitUntil(
    timeout: Duration = .seconds(5),
    condition: () async -> Bool
) async throws {
    let deadline = clock.now.advanced(by: timeout)
    while await condition() == false {
        guard clock.now < deadline else {
            throw BenchmarkFailure.message("timed out awaiting benchmark production-path state")
        }
        await Task.yield()
    }
}

private func waitForRevision(
    _ revision: Revision,
    controller: SessionController
) async throws {
    try await waitUntil {
        controller.applier.lastAppliedRevision == revision
    }
}

private struct CapturedEvent: Sendable {
    let id: Data
    let sequence: UInt64
    let observedRevision: UInt64
}

private func capturedEvents(in transport: BenchmarkTransport) async throws -> [CapturedEvent] {
    let frames = await transport.framesSent()
    return try frames.compactMap { frame in
        let message = try SRUIFraming.decodeFramed(SRUIMessage.self, from: frame.data)
        guard case .event(let event)? = message.msg else { return nil }
        return CapturedEvent(
            id: event.eventID,
            sequence: event.eventSeq,
            observedRevision: event.observedRevision
        )
    }
}

@MainActor
private func startActiveSession(
    transport: BenchmarkTransport,
    renderer: AppKitRenderer,
    fixtureOperations: [SemanticModel.Operation],
    sessionID: String
) async throws -> SessionController {
    let controller = SessionController(transport: transport, renderer: renderer)
    controller.attachRenderer(renderer)
    try await controller.start()
    await controller.handleIncomingMessage(welcomeMessage(sessionID: sessionID))
    let initial = Transaction(baseRevision: Revision(0), operations: fixtureOperations)
    await controller.handleIncomingMessage(try transactionMessage(initial))
    try await waitForRevision(Revision(1), controller: controller)
    guard controller.isEventDispatchEnabled else {
        throw BenchmarkFailure.message("benchmark session did not enable event dispatch")
    }
    return controller
}

@MainActor
private func mutationAndCadence(
    fixtureOperations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) async throws -> Section {
    let baseStore = try makeStore(fixtureOperations)
    var metrics = [Metric]()
    var scalarOnly = true
    var allMutationRastersCompleted = true

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
            allMutationRastersCompleted = allMutationRastersCompleted && result.rasterized
            scalarOnly = scalarOnly && result.classifications.allSatisfy {
                if case .structureAffecting = $0 { return false }
                return true
            }
        }
        let target = count == 100 ? 1.0 : (count == 1_000 ? 5.0 : nil)
        metrics.append(metric("\(count) updates semantic decode/apply", p50(semanticLatencies), target: target))
        metrics.append(metric("\(count) updates semantic decode/apply", percentile(semanticLatencies, 0.95), "ms", "p95"))
        metrics.append(metric("\(count) updates semantic decode/apply", percentile(semanticLatencies, 0.99), "ms", "p99"))
        metrics.append(metric("\(count) updates decode-to-visible", p50(visibleLatencies)))
        metrics.append(metric("\(count) updates decode-to-visible", percentile(visibleLatencies, 0.95), "ms", "p95"))
        metrics.append(metric("\(count) updates decode-to-visible", percentile(visibleLatencies, 0.99), "ms", "p99"))
        metrics.append(metric("\(count) updates wire bytes", Double(bytes), "bytes", "exact"))
        metrics.append(metric("\(count) updates message count", 1, "messages", "exact"))
    }

    var cadenceWire = [Int]()
    var cadenceMessages = [Int]()
    var repaintCounts = [Int]()
    var finalValues = [Double]()
    var idleBytes = [Int]()
    var idleMessages = [Int]()
    var revisionsPreserved = true
    var eventsPreserved = true

    for hz in [60, 120, 144, 240] {
        let renderer = AppKitRenderer()
        let transport = BenchmarkTransport()
        let sessionID = "cadence-\(hz)"
        let controller = try await startActiveSession(
            transport: transport,
            renderer: renderer,
            fixtureOperations: fixtureOperations,
            sessionID: sessionID
        )
        let beforeStream = await transport.snapshot()
        var repaintCount = 0
        var nextPaintTime = 1_000.0 / Double(hz)
        var paintedRevision = Revision(1)
        var observedRevisions = [UInt64]()
        var expectedEvents = [CapturedEvent]()

        for index in 1...24 {
            let transaction = Transaction(
                baseRevision: Revision(UInt64(index)),
                operations: [
                    .setProperty(
                        id: NodeId(5),
                        property: .value,
                        value: .float64(Double(index) / 24.0)
                    )
                ]
            )
            try await transport.injectFromServer(try framed(transactionMessage(transaction)))
            let revision = Revision(UInt64(index + 1))
            try await waitForRevision(revision, controller: controller)
            observedRevisions.append(controller.applier.lastAppliedRevision.value)

            if index.isMultiple(of: 8) {
                let event = try await controller.sendActivate(nodeId: NodeId(16))
                expectedEvents.append(
                    CapturedEvent(
                        id: event.eventId.bytes,
                        sequence: event.eventSeq,
                        observedRevision: event.observedRevision.value
                    )
                )
                await controller.handleIncomingMessage(
                    acknowledgeMessage(
                        event,
                        outbox: controller.outbox,
                        sessionID: sessionID,
                        revision: revision
                    )
                )
            }

            let syntheticTime = Double(index) * 4.0
            if syntheticTime >= nextPaintTime {
                if rasterizeRenderer(renderer, showWindows: fullPaint && repaintCount == 0) {
                    repaintCount += 1
                    paintedRevision = revision
                }
                repeat {
                    nextPaintTime += 1_000.0 / Double(hz)
                } while nextPaintTime <= syntheticTime
            }
        }
        let finalRevision = Revision(25)
        if paintedRevision != finalRevision {
            if rasterizeRenderer(renderer, showWindows: fullPaint && repaintCount == 0) {
                repaintCount += 1
                paintedRevision = finalRevision
            }
        }

        let afterStream = await transport.snapshot()
        let streamBytes = afterStream.inboundBytes - beforeStream.inboundBytes
        let streamMessages = afterStream.inboundMessages - beforeStream.inboundMessages
        cadenceWire.append(streamBytes)
        cadenceMessages.append(streamMessages)
        repaintCounts.append(repaintCount)
        metrics.append(metric("\(hz)Hz wire bytes", Double(streamBytes), "bytes", "exact"))
        metrics.append(metric("\(hz)Hz message count", Double(streamMessages), "messages", "exact"))
        metrics.append(metric("\(hz)Hz repaint count", Double(repaintCount), "repaints", "exact"))

        let progress = renderer.registry.view(for: NodeId(5)) as? NSProgressIndicator
        finalValues.append(progress?.doubleValue ?? -1)
        revisionsPreserved = revisionsPreserved
            && observedRevisions == Array(2...25).map(UInt64.init)
            && paintedRevision == finalRevision
        let wireEvents = try await capturedEvents(in: transport)
        eventsPreserved = eventsPreserved
            && wireEvents.count == expectedEvents.count
            && zip(wireEvents, expectedEvents).allSatisfy {
                $0.id == $1.id
                    && $0.sequence == $1.sequence
                    && $0.observedRevision == $1.observedRevision
            }

        let idleStart = await transport.snapshot()
        pumpRunLoop(for: 0.02)
        await Task.yield()
        let idleEnd = await transport.snapshot()
        idleBytes.append(
            (idleEnd.inboundBytes - idleStart.inboundBytes)
                + (idleEnd.outboundBytes - idleStart.outboundBytes)
        )
        idleMessages.append(
            (idleEnd.inboundMessages - idleStart.inboundMessages)
                + (idleEnd.outboundMessages - idleStart.outboundMessages)
        )

        await controller.stop()
        closeRenderer(renderer)
    }

    metrics.append(metric("idle UI wire bytes", Double(idleBytes.max() ?? -1), "bytes", "observed max"))
    metrics.append(metric("idle UI message count", Double(idleMessages.max() ?? -1), "messages", "observed max"))

    return Section(
        id: "31.3",
        name: "Mutation and frame independence",
        metrics: metrics,
        assertions: [
            Assertion(
                name: "decode-to-visible samples complete actual rasterization",
                passed: allMutationRastersCompleted,
                detail: "every 1/100/1,000-update sample produced an AppKit bitmap"
            ),
            Assertion(
                name: "idle semantic UI emits zero observed SRUI traffic",
                passed: idleBytes.allSatisfy { $0 == 0 } && idleMessages.allSatisfy { $0 == 0 },
                detail: "20ms idle observation deltas: bytes \(idleBytes), messages \(idleMessages)"
            ),
            Assertion(
                name: "wire bytes and message count are cadence independent",
                passed: Set(cadenceWire).count == 1 && Set(cadenceMessages).count == 1,
                detail: "bytes \(cadenceWire), messages \(cadenceMessages)"
            ),
            Assertion(
                name: "local repaint count varies independently",
                passed: Set(repaintCounts).count > 1,
                detail: "synthetic render-clock repaint counts \(repaintCounts)"
            ),
            Assertion(
                name: "coalesced presentation preserves committed revisions, state, and event order",
                passed: Set(finalValues).count == 1
                    && finalValues.first == 1.0
                    && scalarOnly
                    && revisionsPreserved
                    && eventsPreserved,
                detail: "all cadences committed revisions 2...25, emitted ordered events, and rendered progress 1.0"
            ),
        ],
        notes: [
            "A deterministic 4ms mutation source feeds production framing, SessionController, EventOutbox, ProtocolDecoder, SemanticStore, and AppKitRenderer; only bitmap presentation is clocked at 60/120/144/240Hz."
        ]
    )
}

private struct LocalInteractionResult {
    let samples: [String: [Double]]
    let stateChecksPassed: Bool
    let menuMode: String
}

@MainActor
private func rendererTextView(_ renderer: AppKitRenderer) -> NSTextView? {
    guard let handle = renderer.registry.handle(for: NodeId(14)) else { return nil }
    if let view = handle.view as? NSTextView { return view }
    return (handle.view as? NSScrollView)?.documentView as? NSTextView
}

@MainActor
private func localInteractionSamples(
    renderer: AppKitRenderer,
    iterations: Int,
    fullPaint: Bool
) throws -> LocalInteractionResult {
    guard let textView = rendererTextView(renderer),
          let textScroll = renderer.registry.view(for: NodeId(14)) as? NSScrollView,
          let button = renderer.registry.view(for: NodeId(16)) as? NSButton,
          let surface = renderer.registry.handle(for: NodeId(1)),
          let host = surface.window?.contentView else {
        throw BenchmarkFailure.message("representative native interaction controls did not mount")
    }
    if fullPaint {
        surface.window?.makeKeyAndOrderFront(nil)
    }
    textView.frame.size.height = max(textView.frame.height, 5_000)
    let popUp = NSPopUpButton(frame: NSRect(x: 620, y: 12, width: 140, height: 28), pullsDown: false)
    popUp.addItems(withTitles: ["One", "Two", "Three"])
    host.addSubview(popUp)

    var samples: [String: [Double]] = [:]
    var checks = true
    var menuOpened = 0

    func record(_ name: String, _ body: () -> Bool) {
        let start = clock.now
        let stateCorrect = body()
        let pixels = rasterize(host)
        samples[name, default: []].append(milliseconds(start.duration(to: clock.now)))
        checks = checks && stateCorrect && pixels
    }

    for index in 0..<iterations {
        record("text entry") {
            let before = (textView.string as NSString).length
            textView.insertText(
                "x",
                replacementRange: NSRange(location: before, length: 0)
            )
            return (textView.string as NSString).length == before + 1
        }
        record("caret movement") {
            let location = min(index % 4, (textView.string as NSString).length)
            textView.setSelectedRange(NSRange(location: location, length: 0))
            return textView.selectedRange().location == location
                && textView.selectedRange().length == 0
        }
        record("text selection") {
            let length = min(2, (textView.string as NSString).length)
            textView.setSelectedRange(NSRange(location: 0, length: length))
            return textView.selectedRange().length == length
        }
        record("IME composition") {
            textView.setMarkedText(
                "é",
                selectedRange: NSRange(location: 1, length: 0),
                replacementRange: NSRange(location: NSNotFound, length: 0)
            )
            let marked = textView.hasMarkedText()
            textView.unmarkText()
            return marked
        }
        record("scrolling") {
            let target = NSPoint(x: 0, y: min(4_000, Double(index * 23)))
            textScroll.contentView.scroll(to: target)
            textScroll.reflectScrolledClipView(textScroll.contentView)
            return abs(textScroll.contentView.bounds.origin.y - target.y) < 1
        }
        record("hover and pressed") {
            button.highlight(true)
            let highlighted = button.isHighlighted
            button.highlight(false)
            return highlighted && button.isHighlighted == false
        }
        record("menu opening") {
            guard let menu = popUp.menu else { return false }
            if fullPaint {
                DispatchQueue.main.async {
                    menu.cancelTrackingWithoutAnimation()
                }
                _ = menu.popUp(
                    positioning: menu.items.first,
                    at: NSPoint(x: 0, y: popUp.bounds.maxY),
                    in: popUp
                )
                menuOpened += 1
            } else {
                menu.update()
                popUp.selectItem(at: (index + 1) % popUp.numberOfItems)
                menuOpened += 1
            }
            return menuOpened == index + 1
        }
    }

    popUp.removeFromSuperview()
    return LocalInteractionResult(
        samples: samples,
        stateChecksPassed: checks,
        menuMode: fullPaint ? "opened and cancelled through NSMenu tracking" : "prepared offscreen in smoke"
    )
}

@MainActor
private func networkAndLocalInteraction(
    fixtureOperations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) async throws -> Section {
    var metrics = [Metric]()
    var localByRTT = [Int: [String: [Double]]]()
    var dependentByRTT = [Int: [Double]]()
    var allLocalStateChecks = true
    var menuModes = Set<String>()
    var measuredWireBytes = 0
    var measuredWireMessages = 0

    for rtt in [0, 100, 300, 600] {
        let transport = BenchmarkTransport(rttMilliseconds: rtt)
        let renderer = AppKitRenderer()
        let sessionID = "network-\(rtt)"
        let controller = try await startActiveSession(
            transport: transport,
            renderer: renderer,
            fixtureOperations: fixtureOperations,
            sessionID: sessionID
        )

        let interactionHandler = renderer.onInteraction
        renderer.onInteraction = nil
        async let interactionInFlight = controller.sendActivate(nodeId: NodeId(16))
        await Task.yield()
        let local: LocalInteractionResult
        do {
            local = try localInteractionSamples(
                renderer: renderer,
                iterations: iterations,
                fullPaint: fullPaint
            )
        } catch {
            renderer.onInteraction = interactionHandler
            throw error
        }
        renderer.onInteraction = interactionHandler
        let localEvent = try await interactionInFlight
        await controller.handleIncomingMessage(
            acknowledgeMessage(
                localEvent,
                outbox: controller.outbox,
                sessionID: sessionID,
                revision: controller.applier.lastAppliedRevision
            )
        )
        localByRTT[rtt] = local.samples
        allLocalStateChecks = allLocalStateChecks && local.stateChecksPassed
        menuModes.insert(local.menuMode)
        for (name, values) in local.samples.sorted(by: { $0.key < $1.key }) {
            metrics.append(metric("\(name) at \(rtt)ms RTT", p50(values), target: 16.67))
            metrics.append(metric("\(name) at \(rtt)ms RTT", percentile(values, 0.95), "ms", "p95", target: 16.67))
            metrics.append(metric("\(name) at \(rtt)ms RTT", percentile(values, 0.99), "ms", "p99", target: 16.67))
        }

        var dependent = [Double]()
        let responseIterations = max(3, min(7, iterations))
        for sample in 0..<responseIterations {
            let baseRevision = controller.applier.lastAppliedRevision
            let start = clock.now
            let event = try await controller.sendActivate(nodeId: NodeId(16))
            let response = Transaction(
                baseRevision: baseRevision,
                operations: [
                    .setProperty(
                        id: NodeId(5),
                        property: .value,
                        value: .float64(Double(sample + 1) / Double(responseIterations))
                    )
                ]
            )
            try await transport.injectFromServer(try framed(transactionMessage(response)))
            let nextRevision = Revision(baseRevision.value + 1)
            try await waitForRevision(nextRevision, controller: controller)
            guard rasterizeRenderer(renderer, showWindows: false) else {
                throw BenchmarkFailure.message("network response did not rasterize")
            }
            dependent.append(milliseconds(start.duration(to: clock.now)))
            await controller.handleIncomingMessage(
                acknowledgeMessage(
                    event,
                    outbox: controller.outbox,
                    sessionID: sessionID,
                    revision: nextRevision
                )
            )
        }
        dependentByRTT[rtt] = dependent
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", p50(dependent)))
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", percentile(dependent, 0.95), "ms", "p95"))
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", percentile(dependent, 0.99), "ms", "p99"))

        let transportStats = await transport.snapshot()
        measuredWireBytes += transportStats.outboundBytes + transportStats.inboundBytes
        measuredWireMessages += transportStats.outboundMessages + transportStats.inboundMessages
        await controller.stop()
        closeRenderer(renderer)
    }

    let bandwidthPayload = Data(repeating: 0xA5, count: 16_384)
    let bandwidth = BenchmarkTransport(bytesPerSecond: 1_048_576)
    var bandwidthSamples = [Double]()
    for _ in 0..<3 {
        let start = clock.now
        try await bandwidth.send(data: bandwidthPayload, logicalClass: .resource)
        bandwidthSamples.append(milliseconds(start.duration(to: clock.now)))
    }
    let bandwidthStats = await bandwidth.snapshot()
    await bandwidth.close()
    metrics.append(metric("1MiB/s bandwidth-limited 16KiB transfer", p50(bandwidthSamples)))
    metrics.append(metric("1MiB/s bandwidth-limited 16KiB transfer", percentile(bandwidthSamples, 0.95), "ms", "p95"))
    metrics.append(metric("bandwidth-limited delivered bytes", Double(bandwidthStats.outboundBytes), "bytes", "exact"))

    let loss = BenchmarkTransport(dropOutboundOrdinals: [1])
    try await loss.send(data: bandwidthPayload, logicalClass: .ui)
    try await loss.send(data: bandwidthPayload, logicalClass: .ui)
    let lossStats = await loss.snapshot()
    await loss.close()
    metrics.append(metric("deterministic loss attempts", Double(lossStats.outboundAttempts), "messages", "exact"))
    metrics.append(metric("deterministic loss delivered messages", Double(lossStats.outboundMessages), "messages", "exact"))

    let interruption = BenchmarkTransport(interruptOutboundOrdinals: [1])
    let interruptionStart = clock.now
    var interruptionFailed = false
    do {
        try await interruption.send(data: bandwidthPayload, logicalClass: .input)
    } catch TransportError.closed {
        interruptionFailed = true
    }
    let interruptionLatency = milliseconds(interruptionStart.duration(to: clock.now))
    let interruptionStats = await interruption.snapshot()
    await interruption.close()
    metrics.append(metric("controlled transport interruption detection", interruptionLatency))

    metrics.append(metric("measured session wire bytes", Double(measuredWireBytes), "bytes", "exact"))
    metrics.append(metric("measured session wire messages", Double(measuredWireMessages), "messages", "exact"))

    let baseline = localByRTT[0] ?? [:]
    var p50Added = [Double]()
    var p95Added = [Double]()
    var p99Added = [Double]()
    for (rtt, samples) in localByRTT where rtt != 0 {
        for (name, values) in samples {
            guard let base = baseline[name] else { continue }
            p50Added.append(p50(values) - p50(base))
            p95Added.append(percentile(values, 0.95) - percentile(base, 0.95))
            p99Added.append(percentile(values, 0.99) - percentile(base, 0.99))
        }
    }
    let worstP50Added = max(0, p50Added.max() ?? .infinity)
    let worstP95Added = max(0, p95Added.max() ?? .infinity)
    let worstP99Added = max(0, p99Added.max() ?? .infinity)
    metrics.append(metric("maximum RTT-induced local latency delta", worstP50Added, "ms", "p50", target: 16.67))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP95Added, "ms", "p95", target: 16.67))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP99Added, "ms", "p99", target: 16.67))

    let serverTracksRTT = [100, 300, 600].allSatisfy {
        guard let samples = dependentByRTT[$0] else { return false }
        return p50(samples) >= Double($0) * 0.80
    }
    let impairmentsApplied = bandwidthStats.outboundBytes == bandwidthPayload.count * 3
        && lossStats.outboundAttempts == 2
        && lossStats.outboundMessages == 1
        && lossStats.droppedMessages == 1
        && interruptionFailed
        && interruptionStats.interruptions == 1

    return Section(
        id: "31.4",
        name: "Network and local interaction",
        metrics: metrics,
        assertions: [
            Assertion(
                name: "real mounted local interactions do not acquire one RTT",
                passed: worstP95Added < 16.67 && allLocalStateChecks,
                detail: "largest p95 increase was \(String(format: "%.4f", worstP95Added)) ms; state and raster checks \(allLocalStateChecks)"
            ),
            Assertion(
                name: "injected transport RTT affects production server-dependent feedback",
                passed: serverTracksRTT,
                detail: "SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT"
            ),
            Assertion(
                name: "render and local-feedback paths perform no synchronous network RTT",
                passed: worstP99Added < 16.67,
                detail: "largest p99 local delta was \(String(format: "%.4f", worstP99Added)) ms while an event send was in flight"
            ),
            Assertion(
                name: "bandwidth, loss, and interruption controls carried real bytes",
                passed: impairmentsApplied,
                detail: "\(bandwidthStats.outboundBytes) bandwidth bytes; \(lossStats.droppedMessages) dropped frame; \(interruptionStats.interruptions) interrupted send"
            ),
        ],
        notes: [
            "Controls are the mounted renderer TextArea, ScrollView, and Button; the menu is attached to the same surface. \(menuModes.sorted().joined(separator: "; ")).",
            "The deterministic Transport implementation conforms to the production Transport protocol; SessionController, EventOutbox, framing, decoding, semantic apply, and AppKit rasterization remain production paths.",
        ]
    )
}

private let fixturePNG = Data([
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
    0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
    0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0xF6, 0x17, 0x38, 0x55, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
])

private func resourceMetadataMessage(hash: Data) -> SRUIMessage {
    var metadata = SRUIResourceMetadata()
    metadata.resourceHash = hash
    metadata.mediaType = "image/png"
    metadata.encodedLength = UInt64(fixturePNG.count)
    metadata.decodedWidth = 1
    metadata.decodedHeight = 1
    metadata.priority = .normal
    var message = SRUIMessage()
    message.resourceMetadata = metadata
    return message
}

private func resourceChunkMessage(hash: Data, offset: Int, data: Data) -> SRUIMessage {
    var chunk = SRUIResourceChunk()
    chunk.resourceHash = hash
    chunk.byteOffset = UInt64(offset)
    chunk.data = data
    var message = SRUIMessage()
    message.resourceChunk = chunk
    return message
}

private func midResourceReconnectSample() async throws -> (latency: Double, passed: Bool) {
    let hash = Data(SHA256.hash(data: fixturePNG))
    let resourceHash = try ResourceHash(bytes: hash)
    let cache = ResourceCache()
    let outbox = EventOutbox()
    let firstTransport = BenchmarkTransport()
    let first = SessionController(
        transport: firstTransport,
        outbox: outbox,
        resourceCache: cache
    )
    try await first.start()
    await first.handleIncomingMessage(welcomeMessage(sessionID: "resource-session"))
    let midpoint = fixturePNG.count / 2
    await first.handleIncomingMessage(resourceMetadataMessage(hash: hash))
    await first.handleIncomingMessage(
        resourceChunkMessage(
            hash: hash,
            offset: 0,
            data: fixturePNG.prefix(midpoint)
        )
    )
    let partialWasInvisible = await cache.contains(resourceHash) == false
    let partialBytes = await cache.retainedBytes()

    let started = clock.now
    await first.stop()
    let partialBytesAfterDisconnect = await cache.retainedBytes()

    let secondTransport = BenchmarkTransport()
    let second = SessionController(
        transport: secondTransport,
        outbox: outbox,
        resourceCache: cache,
        sessionId: "resource-session"
    )
    try await second.start()
    await second.handleIncomingMessage(resumeOKMessage(sessionID: "resource-session"))
    await second.handleIncomingMessage(resourceMetadataMessage(hash: hash))
    await second.handleIncomingMessage(
        resourceChunkMessage(
            hash: hash,
            offset: 0,
            data: fixturePNG.prefix(midpoint)
        )
    )
    await second.handleIncomingMessage(
        resourceChunkMessage(
            hash: hash,
            offset: midpoint,
            data: fixturePNG.suffix(from: midpoint)
        )
    )
    let committed = await cache.contains(resourceHash)
    let secondWasActive = second.isEventDispatchEnabled
    let latency = milliseconds(started.duration(to: clock.now))
    await second.stop()
    return (
        latency,
        partialWasInvisible
            && partialBytes > 0
            && partialBytesAfterDisconnect == 0
            && secondWasActive
            && committed
    )
}

private func supersededResumeSample() async throws -> (
    oldResponseLatency: Double,
    newResponseLatency: Double,
    passed: Bool,
    detail: String
) {
    let outbox = EventOutbox()
    let seedTransport = BenchmarkTransport()
    let seed = SessionController(transport: seedTransport, outbox: outbox)
    try await seed.start()
    await seed.handleIncomingMessage(welcomeMessage(sessionID: "superseded-session"))
    let pending = try await seed.sendActivate(nodeId: NodeId(16))
    await seed.stop()

    let oldTransport = BenchmarkTransport()
    let oldController = SessionController(
        transport: oldTransport,
        outbox: outbox,
        sessionId: "superseded-session"
    )
    try await oldController.start()

    let newTransport = BenchmarkTransport()
    let newController = SessionController(
        transport: newTransport,
        outbox: outbox,
        sessionId: "superseded-session"
    )
    try await newController.start()

    let oldTransportBefore = await oldTransport.snapshot()
    let oldPendingBefore = await outbox.pendingCount
    let oldRevisionBefore = oldController.applier.lastAppliedRevision
    let oldHandshakeBefore = oldController.isHandshakeComplete
    let oldDispatchBefore = oldController.isEventDispatchEnabled
    let oldStart = clock.now
    await oldController.handleIncomingMessage(
        resumeOKMessage(sessionID: "superseded-session")
    )
    let oldLatency = milliseconds(oldStart.duration(to: clock.now))
    let oldTransportAfter = await oldTransport.snapshot()
    let oldEvents = try await capturedEvents(in: oldTransport)
    let pendingAfterOldResponse = await outbox.pendingCount
    let oldRevisionAfter = oldController.applier.lastAppliedRevision
    let oldHandshakeAfter = oldController.isHandshakeComplete
    let oldDispatchAfter = oldController.isEventDispatchEnabled

    let newStart = clock.now
    await newController.handleIncomingMessage(
        resumeOKMessage(sessionID: "superseded-session")
    )
    try await waitUntil {
        (try? await capturedEvents(in: newTransport).count) == 1
    }
    let newLatency = milliseconds(newStart.duration(to: clock.now))
    let newEvents = try await capturedEvents(in: newTransport)
    let pendingAfterNewResponse = await outbox.pendingCount
    let newDispatchBeforeOldStop = newController.isEventDispatchEnabled
    await oldController.stop()
    let newDispatchAfterOldStop = newController.isEventDispatchEnabled
    let checks = [
        "old_events_empty": oldEvents.isEmpty,
        "old_wire_messages_unchanged":
            oldTransportAfter.outboundMessages == oldTransportBefore.outboundMessages,
        "old_wire_bytes_unchanged":
            oldTransportAfter.outboundBytes == oldTransportBefore.outboundBytes,
        "old_pending_seeded": oldPendingBefore == 1,
        "old_pending_unchanged": pendingAfterOldResponse == oldPendingBefore,
        "old_revision_unchanged": oldRevisionAfter == oldRevisionBefore,
        "old_handshake_unchanged": oldHandshakeAfter == oldHandshakeBefore,
        "old_handshake_incomplete_before": oldHandshakeBefore == false,
        "old_handshake_incomplete_after": oldHandshakeAfter == false,
        "old_dispatch_blocked_before": oldDispatchBefore == false,
        "old_dispatch_blocked_after": oldDispatchAfter == false,
        "new_dispatch_enabled": newDispatchBeforeOldStop,
        "old_stop_did_not_disable_new": newDispatchAfterOldStop,
        "new_replay_count_one": newEvents.count == 1,
        "new_replay_id_matches": newEvents.first.map { $0.id == pending.eventId.bytes } ?? false,
        "new_replay_sequence_matches":
            newEvents.first.map { $0.sequence == pending.eventSeq } ?? false,
        "pending_retained_until_ack": pendingAfterNewResponse == 1,
    ]
    let passed = checks.values.allSatisfy { $0 }
    let detail = checks.keys.sorted().map { "\($0)=\(checks[$0] == true)" }.joined(separator: ", ")
    await newController.stop()
    return (oldLatency, newLatency, passed, detail)
}

private func reconnect(iterations: Int) async throws -> Section {
    var resourceLatencies = [Double]()
    var supersededLatencies = [Double]()
    var activeLatencies = [Double]()
    var resourcePassed = true
    var supersededPassed = true
    var supersededDetails = Set<String>()
    for _ in 0..<max(2, min(5, iterations)) {
        let resource = try await midResourceReconnectSample()
        resourceLatencies.append(resource.latency)
        resourcePassed = resourcePassed && resource.passed

        let superseded = try await supersededResumeSample()
        supersededLatencies.append(superseded.oldResponseLatency)
        activeLatencies.append(superseded.newResponseLatency)
        supersededPassed = supersededPassed && superseded.passed
        supersededDetails.insert(superseded.detail)
    }
    return Section(
        id: "31.5",
        name: "Reconnect",
        metrics: [
            metric("mid-resource reconnect recovery", p50(resourceLatencies)),
            metric("mid-resource reconnect recovery", percentile(resourceLatencies, 0.95), "ms", "p95"),
            metric("superseded resume response handling", p50(supersededLatencies)),
            metric("superseded resume response handling", percentile(supersededLatencies, 0.95), "ms", "p95"),
            metric("active resume response handling", p50(activeLatencies)),
            metric("active resume response handling", percentile(activeLatencies, 0.95), "ms", "p95"),
        ],
        assertions: [
            Assertion(
                name: "mid-resource disconnect discards partial bytes and retransmission commits",
                passed: resourcePassed,
                detail: "SessionController ownership retired invisible partials; replacement replayed metadata and contiguous chunks from offset zero"
            ),
            Assertion(
                name: "superseded resume response is fully inert",
                passed: supersededPassed,
                detail: supersededDetails.sorted().joined(separator: "; ")
            ),
        ],
        notes: [
            "The macOS reconnect driver shares the production EventOutbox and ResourceCache across replacement SessionController instances."
        ]
    )
}

@MainActor
private func terminal(iterations: Int, fullPaint: Bool) async throws -> Section {
    let line = Data("\u{1b}[32mbenchmark output\u{1b}[0m\r\n".utf8)
    var payload = Data()
    payload.reserveCapacity(line.count * 256)
    for _ in 0..<256 { payload.append(line) }

    let session = TerminalSession()
    let streamID = NodeId(14)
    let view = TerminalView(nodeID: streamID)
    let window = NSWindow(
        contentRect: view.frame,
        styleMask: [.titled, .closable, .resizable],
        backing: .buffered,
        defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentView = view
    if fullPaint {
        window.makeKeyAndOrderFront(nil)
    }

    var offset: UInt64 = 0
    var samples = [Double]()
    var finalOffset: UInt64 = 0
    var rasterCompletions = 0
    _ = rasterize(view)
    for _ in 0..<iterations {
        let start = clock.now
        let snapshot = try await session.applyData(
            streamID: streamID,
            byteOffset: offset,
            data: payload
        )
        view.apply(snapshot)
        if rasterize(view) {
            rasterCompletions += 1
        }
        samples.append(milliseconds(start.duration(to: clock.now)))
        offset += UInt64(payload.count)
        finalOffset = snapshot.nextOffset
        await session.acknowledgeRedraw(streamID: streamID)
    }
    window.close()

    return Section(
        id: "31.6",
        name: "Terminal",
        metrics: [
            metric("embedded Terminal decode-to-visible", p50(samples)),
            metric("embedded Terminal decode-to-visible", percentile(samples, 0.95), "ms", "p95"),
            metric("embedded Terminal decode-to-visible", percentile(samples, 0.99), "ms", "p99"),
            metric("embedded terminal frame", Double(payload.count), "bytes", "exact"),
            metric("embedded terminal raster completions", Double(rasterCompletions), "frames", "exact"),
        ],
        assertions: [
            Assertion(
                name: "embedded terminal offsets remain contiguous",
                passed: finalOffset == offset,
                detail: "final offset \(finalOffset)"
            ),
            Assertion(
                name: "embedded terminal visible completion is an actual draw",
                passed: rasterCompletions == iterations,
                detail: "\(rasterCompletions)/\(iterations) TerminalView bitmap completions"
            ),
        ],
        notes: [
            "The embedded measurement uses the same exact ANSI payload as the standalone PTY driver and includes VT parsing, TerminalView snapshot apply, and bitmap draw completion."
        ]
    )
}

@main
private struct BenchmarkDriver {
    @MainActor
    static func main() async throws {
        _ = NSApplication.shared
        let arguments = try Arguments()
        let fixture = try JSONDecoder().decode(
            Fixture.self,
            from: Data(contentsOf: arguments.fixture)
        )
        let fixtureOperations = try operations(for: fixture)
        let iterations = arguments.profile == "full" ? 20 : 3
        let fullPaint = arguments.profile == "full"

        if let candidate = arguments.candidate {
            let result: RendererCandidateResult
            switch candidate {
            case "srui":
                result = try runSRUICandidate(
                    fixture: fixture,
                    operations: fixtureOperations,
                    iterations: iterations,
                    fullPaint: fullPaint
                )
            case "webkit":
                result = try await runWebCandidate(
                    fixture: fixture,
                    iterations: iterations,
                    fullPaint: fullPaint
                )
            default:
                throw BenchmarkFailure.message("unknown renderer candidate \(candidate)")
            }
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try encoder.encode(result).write(to: arguments.output)
            return
        }

        let canonicalTransaction = try Transaction(
            baseRevision: Revision(0),
            operations: fixtureOperations
        ).toWire().serializedData()

        var sections = [Section]()
        if arguments.onlySection == nil || arguments.onlySection == "31.1" {
            sections.append(
                try localRenderer(
                    fixture: fixture,
                    fixtureURL: arguments.fixture,
                    profile: arguments.profile
                )
            )
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.3" {
            sections.append(
                try await mutationAndCadence(
                    fixtureOperations: fixtureOperations,
                    iterations: iterations,
                    fullPaint: fullPaint
                )
            )
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.4" {
            sections.append(
                try await networkAndLocalInteraction(
                    fixtureOperations: fixtureOperations,
                    iterations: max(5, iterations),
                    fullPaint: fullPaint
                )
            )
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.5" {
            sections.append(try await reconnect(iterations: iterations))
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.6" {
            sections.append(
                try await terminal(
                    iterations: max(10, iterations),
                    fullPaint: fullPaint
                )
            )
        }

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(
            Output(
                artifacts: Artifacts(
                    canonicalTransactionSHA256: digestHex(canonicalTransaction),
                    canonicalTransactionBytes: canonicalTransaction.count
                ),
                sections: sections
            )
        ).write(to: arguments.output)
    }
}
