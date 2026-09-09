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

private enum JSONScalar: Codable, Equatable {
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

    func encode(to encoder: any Swift.Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value): try container.encode(value)
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
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
    let rendererProcessAttribution: [RendererProcessAttribution]

    enum CodingKeys: String, CodingKey {
        case canonicalTransactionSHA256 = "canonical_transaction_sha256"
        case canonicalTransactionBytes = "canonical_transaction_bytes"
        case rendererProcessAttribution = "renderer_process_attribution"
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

private struct Assertion: Encodable {
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

private struct Arguments {
    let fixture: URL
    let output: URL
    let profile: String
    let candidate: String?
    let onlySection: String?
    let driverPID: Int32
    let driverBirthUnixNanoseconds: UInt64?

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
        if let index = values.firstIndex(of: "--driver-pid"), values.indices.contains(index + 1),
           let parsed = Int32(values[index + 1]), parsed > 0 {
            driverPID = parsed
        } else {
            driverPID = getpid()
        }
        if candidate != nil {
            guard let index = values.firstIndex(of: "--driver-birth-unix-ns"),
                  values.indices.contains(index + 1),
                  let parsed = UInt64(values[index + 1]),
                  parsed > 0 else {
                throw BenchmarkFailure.message("missing --driver-birth-unix-ns")
            }
            driverBirthUnixNanoseconds = parsed
        } else {
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

private func semanticIdentifier(_ value: String) -> String {
    value.lowercased().split {
        $0.isLetter == false && $0.isNumber == false
    }.joined(separator: ".")
}

private func metric(
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

private struct ParityNode: Codable, Equatable {
    let id: UInt64
    let type: String
    let parent: UInt64?
    let properties: [String: JSONScalar]
}

private struct DOMInspection: Decodable {
    let nodes: [ParityNode]
    let elementKindsPassed: Bool
    let renderedPropertiesPassed: Bool
}

private func fixtureParityNodes(_ fixture: Fixture) -> [ParityNode] {
    fixture.nodes.map {
        ParityNode(
            id: $0.id,
            type: $0.type,
            parent: $0.parent,
            properties: $0.properties ?? [:]
        )
    }.sorted { $0.id < $1.id }
}

private func encodedProperties(_ properties: [String: JSONScalar]) throws -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    return try encoder.encode(properties).base64EncodedString()
}

private func html(for fixture: Fixture) throws -> String {
    let byParent = Dictionary(grouping: fixture.nodes) { $0.parent }
    func children(of node: FixtureNode) throws -> String {
        try (byParent[node.id] ?? []).map(render).joined()
    }
    func attribute(_ name: String, _ value: String?) -> String {
        value.map { " \(name)=\"\(escapedHTML($0))\"" } ?? ""
    }
    func render(_ node: FixtureNode) throws -> String {
        let properties = node.properties ?? [:]
        let metadata = " data-srui-id=\"\(node.id)\""
            + " data-srui-type=\"\(escapedHTML(node.type))\""
            + " data-srui-properties=\"\(try encodedProperties(properties))\""
        let label = properties["label"]?.stringValue
        let text = properties["text"]?.stringValue ?? label ?? ""
        let descendants = try children(of: node)
        switch node.type {
        case "Surface":
            return "<main\(metadata) role=\"application\"\(attribute("aria-label", label))>\(descendants)</main>"
        case "Column":
            return "<section\(metadata) class=\"column\"\(attribute("aria-label", label))>\(descendants)</section>"
        case "Row":
            return "<div\(metadata) class=\"row\"\(attribute("aria-label", label))>\(descendants)</div>"
        case "Text":
            return "<p\(metadata)>\(escapedHTML(text))\(descendants)</p>"
        case "RichText":
            return "<pre\(metadata)>\(escapedHTML(text))\(descendants)</pre>"
        case "Progress":
            let value = properties["value"]?.numberValue ?? 0
            return "<progress\(metadata) max=\"1\" value=\"\(value)\"\(attribute("aria-valuetext", properties["value_description"]?.stringValue))></progress>\(descendants)"
        case "Tree":
            return "<nav\(metadata) role=\"tree\"\(attribute("aria-label", label))>\(descendants)</nav>"
        case "TextArea":
            let value = properties["value"]?.stringValue ?? ""
            return "<textarea\(metadata)\(attribute("aria-label", label))>\(escapedHTML(value))</textarea>\(descendants)"
        case "Button":
            let disabled = properties["enabled"]?.boolValue == false ? " disabled" : ""
            return "<button\(metadata)\(disabled)>\(escapedHTML(label ?? "Button"))</button>\(descendants)"
        case "Separator":
            return "<hr\(metadata)>\(descendants)"
        default:
            return "<div\(metadata) role=\"group\"\(attribute("aria-label", label))>\(escapedHTML(text))\(descendants)</div>"
        }
    }
    let body = try (byParent[nil] ?? []).map(render).joined()
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

private func nativeSemanticParity(store: SemanticStore, fixture: Fixture) throws -> Bool {
    guard store.nodeCount == fixture.nodes.count else { return false }
    for expected in fixture.nodes {
        guard let node = store.getNode(NodeId(expected.id)),
              node.nodeType == (try resolveStandardNodeType(expected.type).get()),
              node.parentID?.value == expected.parent,
              node.properties.count == (expected.properties ?? [:]).count else {
            return false
        }
        for (name, scalar) in expected.properties ?? [:] {
            let property = try resolveStandardProperty(name).get()
            guard node.properties[property] == scalar.semanticValue else { return false }
        }
    }
    return true
}

@MainActor
private func nativeRenderedPropertiesMatch(
    renderer: AppKitRenderer,
    fixture: Fixture
) -> Bool {
    for expected in fixture.nodes {
        guard let handle = renderer.registry.handle(for: NodeId(expected.id)) else {
            return false
        }
        let properties = expected.properties ?? [:]
        if let expectedLabel = properties["label"]?.stringValue,
           handle.accessibilityMetadata.label != expectedLabel {
            return false
        }
        if let expectedValueDescription =
            properties["value_description"]?.stringValue,
           handle.accessibilityMetadata.valueDescription != expectedValueDescription {
            return false
        }
        switch expected.type {
        case "Text":
            guard let expectedText = properties["text"]?.stringValue,
                  let field = handle.view as? NSTextField,
                  field.stringValue == expectedText else {
                return false
            }
        case "RichText":
            let textView = (handle.view as? NSTextView)
                ?? (handle.view as? NSScrollView)?.documentView as? NSTextView
            guard let expectedText = properties["text"]?.stringValue,
                  textView?.string == expectedText else {
                return false
            }
        case "Progress":
            guard let expectedValue = properties["value"]?.numberValue,
                  let progress = handle.view as? NSProgressIndicator,
                  abs(progress.doubleValue - expectedValue) < 0.000_001 else {
                return false
            }
        case "TextArea":
            let textView = (handle.view as? NSTextView)
                ?? (handle.view as? NSScrollView)?.documentView as? NSTextView
            guard let expectedValue = properties["value"]?.stringValue,
                  textView?.string == expectedValue else {
                return false
            }
        case "Button":
            guard let button = handle.view as? NSButton,
                  button.title == (properties["label"]?.stringValue ?? "Button"),
                  button.isEnabled == (properties["enabled"]?.boolValue ?? true) else {
                return false
            }
        default:
            continue
        }
    }
    return true
}

@MainActor
private final class TimedContinuation<Value: Sendable> {
    private var continuation: CheckedContinuation<Value, any Error>?
    private var timeoutTask: Task<Void, Never>?

    init(
        continuation: CheckedContinuation<Value, any Error>,
        operation: String,
        timeout: Duration = .seconds(10)
    ) {
        self.continuation = continuation
        timeoutTask = Task { @MainActor [weak self] in
            do {
                try await Task.sleep(for: timeout)
            } catch {
                return
            }
            self?.finish(.failure(BenchmarkFailure.message("\(operation) timed out")))
        }
    }

    func succeed(_ value: Value) {
        finish(.success(value))
    }

    func fail(_ error: any Error) {
        finish(.failure(error))
    }

    private func finish(_ result: Result<Value, any Error>) {
        guard let continuation else { return }
        self.continuation = nil
        timeoutTask?.cancel()
        timeoutTask = nil
        continuation.resume(with: result)
    }
}

@MainActor
private final class AnimationFrameProbe: NSObject, WKScriptMessageHandler {
    private weak var controller: WKUserContentController?
    private let name: String
    private let gate: TimedContinuation<Void>

    init(
        controller: WKUserContentController,
        name: String,
        gate: TimedContinuation<Void>
    ) {
        self.controller = controller
        self.name = name
        self.gate = gate
    }

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        controller?.removeScriptMessageHandler(forName: name)
        gate.succeed(())
    }

    func fail(_ error: any Error) {
        controller?.removeScriptMessageHandler(forName: name)
        gate.fail(error)
    }
}

@MainActor
private func nextAnimationFrame(in webView: WKWebView) async throws {
    let controller = webView.configuration.userContentController
    let name = "sruiFrame" + UUID().uuidString.replacingOccurrences(of: "-", with: "")
    try await withCheckedThrowingContinuation {
        (continuation: CheckedContinuation<Void, any Error>) in
        let gate = TimedContinuation(
            continuation: continuation,
            operation: "WKWebView requestAnimationFrame"
        )
        let probe = AnimationFrameProbe(controller: controller, name: name, gate: gate)
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
private func snapshotRenderedPixels(_ webView: WKWebView) async throws -> Bool {
    let configuration = WKSnapshotConfiguration()
    configuration.rect = webView.bounds
    return try await withCheckedThrowingContinuation { continuation in
        let gate = TimedContinuation(
            continuation: continuation,
            operation: "WKWebView snapshot"
        )
        webView.takeSnapshot(with: configuration) { image, error in
            if let error {
                gate.fail(error)
            } else if let image {
                gate.succeed(image.size.width > 0 && image.size.height > 0)
            } else {
                gate.fail(BenchmarkFailure.message("WKWebView snapshot returned no image"))
            }
        }
    }
}

@MainActor
private func javascriptString(_ script: String, in webView: WKWebView) async throws -> String {
    try await withCheckedThrowingContinuation { continuation in
        let gate = TimedContinuation(
            continuation: continuation,
            operation: "WKWebView JavaScript evaluation"
        )
        webView.evaluateJavaScript(script) { value, error in
            if let error {
                gate.fail(error)
            } else if let string = value as? String {
                gate.succeed(string)
            } else {
                gate.fail(BenchmarkFailure.message("JavaScript did not return a string"))
            }
        }
    }
}

@MainActor
private func waitForWebContent(
    expectedNodeID: UInt64?,
    requireAnimationFrame: Bool,
    in webView: WKWebView
) async throws {
    if let expectedNodeID {
        let deadline = Date().addingTimeInterval(10)
        var found = false
        var lastError: (any Error)?
        while found == false, Date() < deadline {
            do {
                found = try await javascriptString(
                    "String(document.querySelector('[data-srui-id=\"\(expectedNodeID)\"]') !== null)",
                    in: webView
                ) == "true"
            } catch {
                lastError = error
            }
            if found == false {
                pumpRunLoop(for: 0.002)
                await Task.yield()
            }
        }
        guard found else {
            throw BenchmarkFailure.message(
                "WKWebView representative DOM sentinel \(expectedNodeID) timed out; last JavaScript error: \(String(describing: lastError))"
            )
        }
    }
    if requireAnimationFrame {
        try await nextAnimationFrame(in: webView)
    } else {
        _ = try await javascriptString(
            "String(document.documentElement.getBoundingClientRect().width > 0)",
            in: webView
        )
    }
}

@MainActor
private func inspectDOM(in webView: WKWebView) async throws -> DOMInspection {
    let source = """
    (() => {
      const expectedTags = {
        Surface: "MAIN", Column: "SECTION", Row: "DIV", Text: "P",
        RichText: "PRE", Progress: "PROGRESS", Tree: "NAV",
        TextArea: "TEXTAREA", Button: "BUTTON", Separator: "HR"
      };
      const elements = Array.from(document.querySelectorAll("[data-srui-id]"));
      const propertiesFor = (element) => {
        const bytes = Uint8Array.from(
          atob(element.dataset.sruiProperties),
          (value) => value.charCodeAt(0)
        );
        return JSON.parse(new TextDecoder().decode(bytes));
      };
      const nodes = elements.map((element) => {
        const owner = element.parentElement?.closest("[data-srui-id]") ?? null;
        return {
          id: Number(element.dataset.sruiId),
          type: element.dataset.sruiType,
          parent: owner ? Number(owner.dataset.sruiId) : null,
          properties: propertiesFor(element)
        };
      }).sort((lhs, rhs) => lhs.id - rhs.id);
      const elementKindsPassed = elements.every(
        (element) => expectedTags[element.dataset.sruiType] === element.tagName
      );
      const renderedPropertiesPassed = elements.every((element) => {
        const properties = propertiesFor(element);
        switch (element.dataset.sruiType) {
          case "Text":
          case "RichText":
            return element.textContent === (properties.text ?? properties.label ?? "");
          case "Progress":
            return Math.abs(Number(element.value) - Number(properties.value ?? 0)) < 0.000001
              && (properties.value_description === undefined
                || element.getAttribute("aria-valuetext") === properties.value_description);
          case "TextArea":
            return element.value === (properties.value ?? "")
              && (properties.label === undefined
                || element.getAttribute("aria-label") === properties.label);
          case "Button":
            return element.textContent === (properties.label ?? "Button")
              && element.disabled === (properties.enabled === false);
          case "Surface":
          case "Column":
          case "Row":
          case "Tree":
            return properties.label === undefined
              || element.getAttribute("aria-label") === properties.label;
          default:
            return true;
        }
      });
      return JSON.stringify({nodes, elementKindsPassed, renderedPropertiesPassed});
    })()
    """
    let value = try await javascriptString(source, in: webView)
    return try JSONDecoder().decode(DOMInspection.self, from: Data(value.utf8))
}
private struct RendererCandidateResult: Codable {
    let candidate: String
    let firstPaint: [Double]
    let completePaint: [Double]
    let cpuTime: [Double]
    let hostLiveAllocationDelta: [Double]
    let allocatedFootprintGrowthMiB: Double
    let processLifetimePeak: [Double]
    let renderedNodeCount: Int
    let presentationCompletions: Int
    let representationBytes: Int
    let semanticParityPassed: Bool
    let elementKindsPassed: Bool
    let resourceAttributionComplete: Bool
    let captureAuthorization: Bool
    let pixelCaptureCompletions: Int
    let paintCompletionMode: String
    let attribution: RendererProcessAttribution
    let succeeded: Bool
}

@MainActor
private func observeNativePresentation(
    _ renderer: AppKitRenderer,
    fullPaint: Bool
) async throws -> OnScreenPaintObservation {
    if fullPaint {
        renderer.showWindows()
        let windows = renderer.registry.surfaceHandles.compactMap(\.window)
        guard windows.isEmpty == false else {
            throw BenchmarkFailure.message("native renderer mounted no presentation window")
        }
        var authorized = true
        var pixelVerified = true
        for window in windows {
            let observation = try await benchmarkObserveOnScreenPaint(window)
            guard observation.crossedDisplayRefresh else {
                return OnScreenPaintObservation(
                    crossedDisplayRefresh: false,
                    captureAuthorization: observation.captureAuthorization,
                    pixelCaptureVerified: false
                )
            }
            authorized = authorized && observation.captureAuthorization
            pixelVerified = pixelVerified && observation.pixelCaptureVerified
        }
        return OnScreenPaintObservation(
            crossedDisplayRefresh: true,
            captureAuthorization: authorized,
            pixelCaptureVerified: authorized && pixelVerified
        )
    }
    return OnScreenPaintObservation(
        crossedDisplayRefresh: rasterizeRenderer(renderer, showWindows: false),
        captureAuthorization: false,
        pixelCaptureVerified: false
    )
}

@MainActor
private func verifyNativePresentationPixels(_ renderer: AppKitRenderer) async throws -> Bool {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.isEmpty == false else { return false }
    for window in windows {
        guard try await benchmarkVerifyAuthorizedWindowPixels(window) else {
            return false
        }
    }
    return true
}

@MainActor
private func runSRUICandidate(
    fixture: Fixture,
    operations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool,
    driverPID: Int32
) async throws -> RendererCandidateResult {
    guard let warmOperation = operations.first else {
        throw BenchmarkFailure.message("representative native fixture has no warm operation")
    }
    let warmStore = try makeStore([warmOperation])
    let representativeBytes = try Transaction(
        baseRevision: Revision(0),
        operations: operations
    ).toWire().serializedData()
    let processIDs = [getpid()]
    let startedUnixNanoseconds = benchmarkWallClockNanoseconds()
    var first = [Double]()
    var complete = [Double]()
    var cpu = [Double]()
    var allocations = [Double]()
    var growth = [Double]()
    var peaks = [Double]()
    var renderedNodeCount = 0
    var presentationCompletions = 0
    var captureAuthorization = fullPaint
    var semanticParityPassed = true
    var renderedPropertiesPassed = true
    var resourceAttributionComplete = true
    var verificationRenderer: AppKitRenderer?

    for index in 0..<iterations {
        let renderer = AppKitRenderer()
        try renderer.attach(store: warmStore)
        let warmObservation = try await observeNativePresentation(
            renderer,
            fullPaint: fullPaint
        )
        guard warmObservation.crossedDisplayRefresh else {
            throw BenchmarkFailure.message("native sample renderer did not warm")
        }
        try renderer.attach(store: SemanticStore())

        let beforeAllocator = mallocSample()
        let beforeResources = benchmarkProcessResourceSample(pids: processIDs)
        let started = clock.now
        let wireTransaction = try SRUITransaction(serializedBytes: representativeBytes)
        let transaction = try ProtocolDecoder().validateAndConvertTransaction(
            wire: wireTransaction
        )
        var store = SemanticStore()
        guard case .success = store.applyTransactionRecord(transaction) else {
            throw BenchmarkFailure.message(
                "canonical native transaction failed semantic application"
            )
        }
        try renderer.attach(store: store)
        for surface in renderer.registry.surfaceHandles {
            surface.window?.contentView?.layoutSubtreeIfNeeded()
        }
        let firstObservation = try await observeNativePresentation(
            renderer,
            fullPaint: fullPaint
        )
        first.append(milliseconds(started.duration(to: clock.now)))
        await Task.yield()
        let completeObservation = try await observeNativePresentation(
            renderer,
            fullPaint: fullPaint
        )
        complete.append(milliseconds(started.duration(to: clock.now)))
        let afterResources = benchmarkProcessResourceSample(pids: processIDs)
        let afterAllocator = mallocSample()

        cpu.append(max(0, afterResources.cpuMilliseconds - beforeResources.cpuMilliseconds))
        allocations.append(Double(max(0, afterAllocator.blocks - beforeAllocator.blocks)))
        growth.append(max(
            0,
            afterResources.physicalFootprintMiB - beforeResources.physicalFootprintMiB
        ))
        peaks.append(afterResources.lifetimePeakPhysicalFootprintMiB)
        resourceAttributionComplete = resourceAttributionComplete
            && beforeResources.measuredPIDCount == processIDs.count
            && afterResources.measuredPIDCount == processIDs.count
        captureAuthorization = captureAuthorization
            && firstObservation.captureAuthorization
            && completeObservation.captureAuthorization
        let sampleSemanticParity = try nativeSemanticParity(
            store: store,
            fixture: fixture
        )
        semanticParityPassed = semanticParityPassed && sampleSemanticParity
        renderedPropertiesPassed = renderedPropertiesPassed
            && nativeRenderedPropertiesMatch(renderer: renderer, fixture: fixture)
        renderedNodeCount = renderer.registry.allHandles.count
        if firstObservation.crossedDisplayRefresh
            && completeObservation.crossedDisplayRefresh {
            presentationCompletions += 1
        }

        if index == iterations - 1, fullPaint, captureAuthorization {
            verificationRenderer = renderer
        } else {
            closeRenderer(renderer)
        }
        withExtendedLifetime(renderer) {}
    }

    let endedUnixNanoseconds = benchmarkWallClockNanoseconds()
    var pixelCaptureCompletions = 0
    if let verificationRenderer {
        if try await verifyNativePresentationPixels(verificationRenderer) {
            pixelCaptureCompletions = 1
        }
        closeRenderer(verificationRenderer)
    }
    guard let hostIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message("native candidate process birth identity was unavailable")
    }
    let attribution = RendererProcessAttribution(
        candidate: "srui",
        driverPID: driverPID,
        hostPID: getpid(),
        helperPIDs: [],
        processIdentities: [hostIdentity],
        startedUnixNanoseconds: startedUnixNanoseconds,
        endedUnixNanoseconds: endedUnixNanoseconds,
        helperPIDSource: "native candidate has no renderer helper processes"
    )
    return RendererCandidateResult(
        candidate: "srui",
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        hostLiveAllocationDelta: allocations,
        allocatedFootprintGrowthMiB: p50(growth),
        processLifetimePeak: peaks,
        renderedNodeCount: renderedNodeCount,
        presentationCompletions: presentationCompletions,
        representationBytes: representativeBytes.count,
        semanticParityPassed: semanticParityPassed,
        elementKindsPassed: renderedPropertiesPassed,
        resourceAttributionComplete: resourceAttributionComplete,
        captureAuthorization: captureAuthorization,
        pixelCaptureCompletions: pixelCaptureCompletions,
        paintCompletionMode: fullPaint
            ? "window submitted and display framebuffer advanced"
            : "offscreen AppKit bitmap fallback",
        attribution: attribution,
        succeeded: renderedNodeCount == fixture.nodes.count
            && presentationCompletions == iterations
            && semanticParityPassed
            && renderedPropertiesPassed
            && resourceAttributionComplete
    )
}

@MainActor
private func loadAndObserveWebView(
    _ webView: WKWebView,
    window: NSWindow,
    probe: NavigationProbe,
    html: String,
    fullPaint: Bool,
    expectedParity: [ParityNode],
    startedAt: ContinuousClock.Instant? = nil
) async throws -> (
    first: Double,
    complete: Double,
    presented: Bool,
    nodeCount: Int,
    parityPassed: Bool,
    elementKindsPassed: Bool,
    captureAuthorization: Bool,
    pixelCaptureVerified: Bool
) {
    probe.reset()
    let phase = expectedParity.isEmpty
        ? "reset"
        : (expectedParity.count == 1 ? "warm" : "representative")
    let started = startedAt ?? clock.now
    webView.loadHTMLString(html, baseURL: nil)
    if expectedParity.isEmpty {
        guard pumpWebView(
            webView,
            until: { probe.finished != nil || probe.failure != nil },
            timeout: 10
        ), probe.failure == nil else {
            throw probe.failure ?? BenchmarkFailure.message(
                "WKWebView \(phase) reset navigation timed out"
            )
        }
    }
    try await waitForWebContent(
        expectedNodeID: expectedParity.first?.id,
        requireAnimationFrame: fullPaint,
        in: webView
    )

    let firstPresented: Bool
    var captureAuthorization = false
    var pixelCaptureVerified = false
    if fullPaint {
        let observation = try await benchmarkObserveOnScreenPaint(window)
        firstPresented = observation.crossedDisplayRefresh
        captureAuthorization = observation.captureAuthorization
        pixelCaptureVerified = observation.pixelCaptureVerified
    } else {
        firstPresented = try await snapshotRenderedPixels(webView)
    }
    let first = milliseconds(started.duration(to: clock.now))

    guard pumpWebView(
        webView,
        until: { probe.finished != nil || probe.failure != nil },
        timeout: 10
    ), probe.failure == nil else {
        throw probe.failure ?? BenchmarkFailure.message(
            "WKWebView \(phase) navigation timed out"
        )
    }
    let completePresented: Bool
    if fullPaint {
        try await nextAnimationFrame(in: webView)
        let observation = try await benchmarkObserveOnScreenPaint(window)
        completePresented = observation.crossedDisplayRefresh
        captureAuthorization = captureAuthorization && observation.captureAuthorization
        pixelCaptureVerified = pixelCaptureVerified && observation.pixelCaptureVerified
    } else {
        completePresented = try await snapshotRenderedPixels(webView)
    }
    let complete = milliseconds(started.duration(to: clock.now))
    let inspection = try await inspectDOM(in: webView)
    return (
        first,
        complete,
        firstPresented && completePresented,
        inspection.nodes.count,
        inspection.nodes == expectedParity,
        inspection.elementKindsPassed && inspection.renderedPropertiesPassed,
        captureAuthorization,
        pixelCaptureVerified
    )
}

@MainActor
private func runWebCandidate(
    fixture: Fixture,
    iterations: Int,
    fullPaint: Bool,
    driverPID: Int32
) async throws -> RendererCandidateResult {
    let representation = try html(for: fixture)
    let expectedParity = fixtureParityNodes(fixture)
    let warmRepresentation = "<!doctype html><html><body><main data-srui-id=\"0\" data-srui-type=\"Surface\" data-srui-properties=\"e30=\" role=\"application\"></main></body></html>"
    let warmParity = [
        ParityNode(id: 0, type: "Surface", parent: nil, properties: [:])
    ]
    let resetRepresentation = "<!doctype html><html><body></body></html>"
    guard let hostIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message("WebKit candidate process birth identity was unavailable")
    }

    let startedUnixNanoseconds = benchmarkWallClockNanoseconds()
    var helperPIDs = Set<pid_t>()
    var processIdentities = [pid_t: RendererProcessIdentity](
        uniqueKeysWithValues: [(getpid(), hostIdentity)]
    )
    var first = [Double]()
    var complete = [Double]()
    var cpu = [Double]()
    var allocations = [Double]()
    var growth = [Double]()
    var peaks = [Double]()
    var renderedNodeCount = 0
    var presentationCompletions = 0
    var captureAuthorization = fullPaint
    var semanticParityPassed = true
    var elementKindsPassed = true
    var resourceAttributionComplete = true
    var verificationWebView: WKWebView?
    var verificationWindow: NSWindow?

    for index in 0..<iterations {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let webView = WKWebView(
            frame: NSRect(x: 0, y: 0, width: 960, height: 720),
            configuration: configuration
        )
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
            NSApplication.shared.activate(ignoringOtherApps: true)
            pumpRunLoop(for: 0.02)
        }

        let warmResult = try await loadAndObserveWebView(
            webView,
            window: window,
            probe: probe,
            html: warmRepresentation,
            fullPaint: fullPaint,
            expectedParity: warmParity
        )
        guard warmResult.presented,
              warmResult.parityPassed,
              warmResult.elementKindsPassed else {
            throw BenchmarkFailure.message("WebKit sample view did not warm")
        }
        let resetResult = try await loadAndObserveWebView(
            webView,
            window: window,
            probe: probe,
            html: resetRepresentation,
            fullPaint: fullPaint,
            expectedParity: []
        )
        guard resetResult.presented, resetResult.nodeCount == 0 else {
            throw BenchmarkFailure.message("WebKit sample view did not reset")
        }

        let beforeHelperPIDs = Set(benchmarkWebKitHelperProcessIDs(webView))
            .subtracting([getpid()])
        let beforeHelperIdentities = beforeHelperPIDs.compactMap {
            benchmarkProcessIdentity(pid: $0)
        }
        var helperBaselines = [pid_t: (RendererProcessIdentity, ProcessResourceSample)]()
        var sampleResourcesComplete =
            beforeHelperIdentities.count == beforeHelperPIDs.count
        for identity in beforeHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
            let resource = benchmarkProcessResourceSample(pids: [identity.pid])
            if resource.measuredPIDCount == 1 {
                helperBaselines[identity.pid] = (identity, resource)
            } else {
                sampleResourcesComplete = false
            }
        }
        helperPIDs.formUnion(beforeHelperPIDs)

        let beforeAllocator = mallocSample()
        let beforeHost = benchmarkProcessResourceSample(pids: [getpid()])
        let sampleStartedUnixNanoseconds = benchmarkWallClockNanoseconds()
        let sampleStarted = clock.now
        let result = try await loadAndObserveWebView(
            webView,
            window: window,
            probe: probe,
            html: representation,
            fullPaint: fullPaint,
            expectedParity: expectedParity,
            startedAt: sampleStarted
        )

        let afterHelperPIDs = Set(benchmarkWebKitHelperProcessIDs(webView))
            .subtracting([getpid()])
        let sampleWebContentPID = benchmarkWebKitWebContentProcessID(webView)
        let afterHelperIdentities = afterHelperPIDs.compactMap {
            benchmarkProcessIdentity(pid: $0)
        }
        sampleResourcesComplete = sampleResourcesComplete
            && afterHelperIdentities.count == afterHelperPIDs.count
            && beforeHelperPIDs.subtracting(afterHelperPIDs).isEmpty
        var helperCPUMilliseconds = 0.0
        var helperFootprintGrowthMiB = 0.0
        var helperLifetimePeakMiB = 0.0
        for identity in afterHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
            let after = benchmarkProcessResourceSample(pids: [identity.pid])
            guard after.measuredPIDCount == 1 else {
                sampleResourcesComplete = false
                continue
            }
            helperLifetimePeakMiB += after.lifetimePeakPhysicalFootprintMiB
            if let (baselineIdentity, baseline) = helperBaselines[identity.pid],
               baselineIdentity.birthUnixNanoseconds == identity.birthUnixNanoseconds {
                helperCPUMilliseconds += max(
                    0,
                    after.cpuMilliseconds - baseline.cpuMilliseconds
                )
                helperFootprintGrowthMiB += max(
                    0,
                    after.physicalFootprintMiB - baseline.physicalFootprintMiB
                )
            } else {
                let birthToleranceNanoseconds: UInt64 = 1_000_000
                let bornDuringSample = identity.birthUnixNanoseconds
                    .addingReportingOverflow(birthToleranceNanoseconds)
                guard bornDuringSample.overflow == false,
                      bornDuringSample.partialValue >= sampleStartedUnixNanoseconds else {
                    sampleResourcesComplete = false
                    continue
                }
                helperCPUMilliseconds += after.cpuMilliseconds
                helperFootprintGrowthMiB += after.physicalFootprintMiB
            }
        }
        helperPIDs.formUnion(afterHelperPIDs)
        let afterHost = benchmarkProcessResourceSample(pids: [getpid()])
        let afterAllocator = mallocSample()

        first.append(result.first)
        complete.append(result.complete)
        cpu.append(
            max(0, afterHost.cpuMilliseconds - beforeHost.cpuMilliseconds)
                + helperCPUMilliseconds
        )
        allocations.append(Double(max(0, afterAllocator.blocks - beforeAllocator.blocks)))
        growth.append(max(
            0,
            afterHost.physicalFootprintMiB - beforeHost.physicalFootprintMiB
                + helperFootprintGrowthMiB
        ))
        peaks.append(
            afterHost.lifetimePeakPhysicalFootprintMiB + helperLifetimePeakMiB
        )
        renderedNodeCount = result.nodeCount
        semanticParityPassed = semanticParityPassed && result.parityPassed
        elementKindsPassed = elementKindsPassed && result.elementKindsPassed
        let webContentIdentified = sampleWebContentPID.map { pid in
            afterHelperIdentities.contains { $0.pid == pid }
        } ?? false
        resourceAttributionComplete = resourceAttributionComplete
            && afterHelperPIDs.isEmpty == false
            && webContentIdentified
            && sampleResourcesComplete
            && beforeHost.measuredPIDCount == 1
            && afterHost.measuredPIDCount == 1
        captureAuthorization = captureAuthorization && result.captureAuthorization
        if result.presented { presentationCompletions += 1 }

        if index == iterations - 1, fullPaint, captureAuthorization {
            verificationWebView = webView
            verificationWindow = window
        } else {
            webView.stopLoading()
            webView.navigationDelegate = nil
            window.contentView = nil
            window.close()
            pumpRunLoop(for: 0.05)
        }
        withExtendedLifetime(webView) {}
    }

    let endedUnixNanoseconds = benchmarkWallClockNanoseconds()
    var pixelCaptureCompletions = 0
    if let verificationWebView, let verificationWindow {
        if try await benchmarkVerifyAuthorizedWindowPixels(verificationWindow) {
            pixelCaptureCompletions = 1
        }
        verificationWebView.stopLoading()
        verificationWebView.navigationDelegate = nil
        verificationWindow.contentView = nil
        verificationWindow.close()
    }
    let attributedPIDs = Set([getpid()]).union(helperPIDs)
    resourceAttributionComplete = resourceAttributionComplete
        && Set(processIdentities.keys) == attributedPIDs
    let attribution = RendererProcessAttribution(
        candidate: "webkit",
        driverPID: driverPID,
        hostPID: getpid(),
        helperPIDs: helperPIDs.sorted(),
        processIdentities: processIdentities.values.sorted { $0.pid < $1.pid },
        startedUnixNanoseconds: startedUnixNanoseconds,
        endedUnixNanoseconds: endedUnixNanoseconds,
        helperPIDSource: "required WebContent PID from benchmark-only _webProcessIdentifier; optional _networkProcessIdentifier/_gpuProcessIdentifier values included when available; unavailable identifiers omitted, never inferred"
    )
    return RendererCandidateResult(
        candidate: "webkit",
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        hostLiveAllocationDelta: allocations,
        allocatedFootprintGrowthMiB: p50(growth),
        processLifetimePeak: peaks,
        renderedNodeCount: renderedNodeCount,
        presentationCompletions: presentationCompletions,
        representationBytes: representation.utf8.count,
        semanticParityPassed: semanticParityPassed,
        elementKindsPassed: elementKindsPassed,
        resourceAttributionComplete: resourceAttributionComplete,
        captureAuthorization: captureAuthorization,
        pixelCaptureCompletions: pixelCaptureCompletions,
        paintCompletionMode: fullPaint
            ? "representative DOM sentinel and animation frame preceded each window submission and display framebuffer advance"
            : "representative DOM sentinel and forced layout preceded each offscreen WKSnapshot fallback",
        attribution: attribution,
        succeeded: renderedNodeCount == fixture.nodes.count
            && presentationCompletions == iterations
            && semanticParityPassed
            && elementKindsPassed
            && resourceAttributionComplete
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
    guard let driverIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message("benchmark driver birth identity was unavailable")
    }
    let process = Process()
    process.executableURL = executable
    process.arguments = [
        "--fixture", fixture.path,
        "--output", temporary.path,
        "--profile", profile,
        "--candidate", name,
        "--driver-pid", String(getpid()),
        "--driver-birth-unix-ns", String(driverIdentity.birthUnixNanoseconds),
    ]
    try process.run()
    let processID = process.processIdentifier
    var launchedIdentity = benchmarkProcessIdentity(pid: processID)
    let identityDeadline = Date().addingTimeInterval(1)
    while launchedIdentity == nil, process.isRunning, Date() < identityDeadline {
        _ = RunLoop.current.run(
            mode: .default,
            before: min(identityDeadline, Date().addingTimeInterval(0.005))
        )
        launchedIdentity = benchmarkProcessIdentity(pid: processID)
    }
    guard let launchedIdentity else {
        throw BenchmarkFailure.message(
            "renderer candidate \(name) birth identity was unavailable; no numeric PID was signalled because ownership could not be validated"
        )
    }

    func waitForCandidateExit(until deadline: Date) -> Bool {
        while process.isRunning && Date() < deadline {
            _ = RunLoop.current.run(
                mode: .default,
                before: min(deadline, Date().addingTimeInterval(0.01))
            )
        }
        return process.isRunning == false
    }

    let deadline = Date().addingTimeInterval(profile == "full" ? 240 : 60)
    guard waitForCandidateExit(until: deadline) else {
        let groupKilled = benchmarkSignalLiveProcessGroup(
            processID,
            expectedBirthUnixNanoseconds: launchedIdentity.birthUnixNanoseconds,
            signal: SIGKILL
        )
        let reaped = waitForCandidateExit(until: Date().addingTimeInterval(5))
        guard reaped else {
            throw BenchmarkFailure.message(
                "renderer candidate \(name) timed out and could not be reaped"
            )
        }
        guard groupKilled else {
            throw BenchmarkFailure.message(
                "renderer candidate \(name) timed out; process group ownership could not be validated before cleanup"
            )
        }
        throw BenchmarkFailure.message("renderer candidate \(name) timed out")
    }

    guard process.terminationReason == .exit, process.terminationStatus == 0 else {
        throw BenchmarkFailure.message(
            "renderer candidate \(name) exited with status \(process.terminationStatus)"
        )
    }
    let result = try JSONDecoder().decode(
        RendererCandidateResult.self,
        from: Data(contentsOf: temporary)
    )
    let attributedPIDs = Set([result.attribution.hostPID] + result.attribution.helperPIDs)
    let identityPIDs = Set(result.attribution.processIdentities.map(\.pid))
    guard result.attribution.hostPID == processID,
          result.attribution.driverPID == getpid(),
          attributedPIDs == identityPIDs,
          result.attribution.processIdentities.allSatisfy({ $0.birthUnixNanoseconds > 0 }),
          result.attribution.processIdentities.first(where: {
            $0.pid == processID
          })?.birthUnixNanoseconds == launchedIdentity.birthUnixNanoseconds else {
        throw BenchmarkFailure.message("renderer candidate process attribution mismatch")
    }
    return result
}

private struct LocalRendererResult {
    let section: Section
    let attributions: [RendererProcessAttribution]
}

@MainActor
private func localRenderer(
    fixture: Fixture,
    fixtureURL: URL,
    profile: String
) throws -> LocalRendererResult {
    let srui = try runCandidateSubprocess(name: "srui", fixture: fixtureURL, profile: profile)
    let web = try runCandidateSubprocess(name: "webkit", fixture: fixtureURL, profile: profile)
    let fullPaint = profile == "full"
    let nativeFirstName = fullPaint
        ? "SRUI first on-screen paint crossing display refresh"
        : "SRUI first offscreen raster fallback"
    let nativeCompleteName = fullPaint
        ? "SRUI complete on-screen paint crossing display refresh"
        : "SRUI complete offscreen raster fallback"
    let webFirstName = fullPaint
        ? "WKWebView first on-screen paint crossing display refresh"
        : "WKWebView first offscreen snapshot fallback"
    let webCompleteName = fullPaint
        ? "WKWebView complete on-screen paint crossing display refresh"
        : "WKWebView complete offscreen snapshot fallback"
    let captureAuthorization = srui.captureAuthorization && web.captureAuthorization

    let section = Section(
        id: "31.1",
        name: "Local renderer",
        metrics: [
            metric(nativeFirstName, p50(srui.firstPaint), id: "srui.first_paint"),
            metric(nativeFirstName, percentile(srui.firstPaint, 0.95), "ms", "p95", id: "srui.first_paint"),
            metric(nativeCompleteName, p50(srui.completePaint), id: "srui.complete_paint"),
            metric(nativeCompleteName, percentile(srui.completePaint, 0.95), "ms", "p95", id: "srui.complete_paint"),
            metric(nativeCompleteName, percentile(srui.completePaint, 0.99), "ms", "p99", id: "srui.complete_paint"),
            metric("SRUI candidate process CPU time", p50(srui.cpuTime), id: "srui.cpu"),
            metric("SRUI candidate process CPU time", percentile(srui.cpuTime, 0.95), "ms", "p95", id: "srui.cpu"),
            metric("SRUI host retained allocation delta", p50(srui.hostLiveAllocationDelta), "allocations", id: "srui.host_retained_allocations"),
            metric("SRUI host allocated footprint growth", srui.allocatedFootprintGrowthMiB, "MiB", "last-first", id: "srui.process_footprint_growth"),
            metric("SRUI lifetime process footprint peak", srui.processLifetimePeak.max() ?? -1, "MiB", "max", id: "srui.process_footprint_peak"),
            metric(webFirstName, p50(web.firstPaint), id: "webkit.first_paint"),
            metric(webFirstName, percentile(web.firstPaint, 0.95), "ms", "p95", id: "webkit.first_paint"),
            metric(webCompleteName, p50(web.completePaint), id: "webkit.complete_paint"),
            metric(webCompleteName, percentile(web.completePaint, 0.95), "ms", "p95", id: "webkit.complete_paint"),
            metric("WKWebView host plus attributed helper CPU time", p50(web.cpuTime), id: "webkit.cpu"),
            metric("WKWebView host retained allocation delta", p50(web.hostLiveAllocationDelta), "allocations", id: "webkit.host_retained_allocations"),
            metric("WKWebView host plus helper allocated footprint growth", web.allocatedFootprintGrowthMiB, "MiB", "last-first", id: "webkit.process_footprint_growth"),
            metric("WKWebView host plus helper lifetime footprint peak", web.processLifetimePeak.max() ?? -1, "MiB", "max", id: "webkit.process_footprint_peak"),
            metric("SRUI representation", Double(srui.representationBytes), "bytes", "exact", id: "representation.srui_bytes"),
            metric("HTML representation", Double(web.representationBytes), "bytes", "exact", id: "representation.html_bytes"),
            metric("screen capture authorization", captureAuthorization ? 1 : 0, "boolean", "exact", id: "paint.capture_authorization"),
        ],
        assertions: [
            Assertion(
                id: "semantic_representation_parity",
                name: "representative fixture preserves exact parent, type, and property semantics",
                passed: srui.semanticParityPassed
                    && srui.elementKindsPassed
                    && web.semanticParityPassed
                    && web.elementKindsPassed,
                detail: "\(srui.renderedNodeCount) native store nodes and rendered control values plus \(web.renderedNodeCount) DOM nodes/elements/rendered properties matched exact fixture records"
            ),
            Assertion(
                id: "paint_completion_observed",
                name: fullPaint
                    ? "first and complete window submissions each advance the display framebuffer"
                    : "smoke fallback first and complete raster outputs are observed offscreen",
                passed: srui.succeeded && web.succeeded,
                detail: "\(srui.presentationCompletions) native and \(web.presentationCompletions) WebKit completions; \(srui.paintCompletionMode); \(web.paintCompletionMode)"
            ),
            Assertion(
                id: "webkit_helpers_attributed",
                name: "WebKit helper resources use exact measured process attribution",
                passed: web.attribution.helperPIDs.isEmpty == false
                    && web.resourceAttributionComplete,
                detail: "host \(web.attribution.hostPID), helpers \(web.attribution.helperPIDs); no process-name matching"
            ),
        ],
        notes: [
            fullPaint
                ? "Full timing requires visible, unoccluded exact CGWindow presence and, for each submitted state, displayIfNeeded/CATransaction flush followed by an NSScreen framebuffer timestamp advance. One optional preauthorized ScreenCaptureKit pixel check per candidate runs after the timed/resource-attribution interval: native=\(srui.pixelCaptureCompletions), WebKit=\(web.pixelCaptureCompletions), capture_authorization=\(captureAuthorization). No permission request is issued."
                : "Smoke deliberately uses named offscreen AppKit bitmap and WKSnapshot fallbacks; it does not claim compositor-visible paint.",
            "Every measured sample uses a fresh candidate instance. That same instance first renders a tiny neutral representation, resets outside timing, takes CPU/allocation baselines, then consumes the representative in-memory transaction bytes or HTML through presentation. Native timing includes protobuf decode, ProtocolDecoder validation, TransactionApplier-equivalent semantic commit, renderer mount, and paint; WebKit waits for the representative DOM sentinel and, in full mode, requestAnimationFrame before presentation.",
            "Each candidate runs in a fresh verified process group. WebKit CPU and allocated-footprint metrics aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs; retained malloc block counts are explicitly host-only.",
        ]
    )
    return LocalRendererResult(
        section: section,
        attributions: [srui.attribution, web.attribution]
    )
}
@MainActor
private func mutationRun(
    baseStore: SemanticStore,
    fixtureOperations: [SemanticModel.Operation],
    operations: [SemanticModel.Operation],
    fullPaint: Bool
) async throws -> (
    semanticLatency: Double,
    visibleLatency: Double,
    bytes: Int,
    messages: Int,
    classifications: [DirtyClassification],
    rasterized: Bool,
    stateParity: Bool
) {
    let transaction = Transaction(baseRevision: baseStore.revision, operations: operations)
    let wireBytes = try framed(transactionMessage(transaction))

    let semanticApplier = TransactionApplier(store: baseStore)
    let semanticStart = clock.now
    let decodedMessage = try SRUIFraming.decodeFramed(SRUIMessage.self, from: wireBytes)
    guard case .transaction(let wireTransaction)? = decodedMessage.msg else {
        throw BenchmarkFailure.message("framed mutation did not contain a transaction")
    }
    let decoded = try ProtocolDecoder().validateAndConvertTransaction(
        wire: wireTransaction
    )
    guard case .success = semanticApplier.apply(record: decoded) else {
        throw BenchmarkFailure.message(
            "mutation transaction did not apply through TransactionApplier"
        )
    }
    let semanticLatency = milliseconds(semanticStart.duration(to: clock.now))
    let semanticSnapshot = semanticApplier.currentSnapshot
    let classifications = DirtyClassifier.classify(decoded)
    guard case .float64(let expectedProgress)? =
        semanticSnapshot.store.getNode(NodeId(5))?.properties[.value] else {
        throw BenchmarkFailure.message("semantic mutation produced no progress value")
    }

    let transport = BenchmarkTransport()
    let renderer = AppKitRenderer()
    let controller = try await startActiveSession(
        transport: transport,
        renderer: renderer,
        fixtureOperations: fixtureOperations,
        sessionID: "mutation-\(operations.count)"
    )
    do {
        let warmObservation = try await observeNativePresentation(
            renderer,
            fullPaint: fullPaint
        )
        guard warmObservation.crossedDisplayRefresh else {
            throw BenchmarkFailure.message("steady-state mutation renderer did not warm")
        }
        let beforeWire = await transport.snapshot()
        let visibleStart = clock.now
        try await transport.injectFromServer(wireBytes)
        let expectedRevision = Revision(baseStore.revision.value + 1)
        try await waitForRevision(expectedRevision, controller: controller)
        try await waitUntil {
            guard let progress = renderer.registry.view(for: NodeId(5))
                as? NSProgressIndicator else {
                return false
            }
            return abs(progress.doubleValue - expectedProgress) < 0.000_001
        }
        let visibleObservation = try await observeNativePresentation(
            renderer,
            fullPaint: fullPaint
        )
        let visibleLatency = milliseconds(visibleStart.duration(to: clock.now))
        let afterWire = await transport.snapshot()
        let productionSnapshot = controller.applier.currentSnapshot
        let stateParity = productionSnapshot.revision == semanticSnapshot.revision
            && productionSnapshot.store.getNode(NodeId(5))?.properties[.value]
                == semanticSnapshot.store.getNode(NodeId(5))?.properties[.value]
        let measuredBytes = afterWire.inboundBytes - beforeWire.inboundBytes
        let measuredMessages = afterWire.inboundMessages - beforeWire.inboundMessages
        await controller.stop()
        closeRenderer(renderer)
        return (
            semanticLatency,
            visibleLatency,
            measuredBytes,
            measuredMessages,
            classifications,
            visibleObservation.crossedDisplayRefresh,
            stateParity && measuredBytes == wireBytes.count && measuredMessages == 1
        )
    } catch {
        await controller.stop()
        closeRenderer(renderer)
        throw error
    }
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
    let activeDelayedOperations: Int
    let closeCalls: Int
    let isClosed: Bool
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
    private var activeDelayedOperations = 0
    private var closeCalls = 0

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
            closed = true
            continuation.finish(throwing: TransportError.closed)
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
        closeCalls += 1
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
            interruptions: interruptions,
            activeDelayedOperations: activeDelayedOperations,
            closeCalls: closeCalls,
            isClosed: closed
        )
    }

    private func applyDelay(byteCount: Int) async throws {
        let serializationMilliseconds = bytesPerSecond.map {
            Double(byteCount) / Double($0) * 1_000.0
        } ?? 0
        let total = oneWayDelayMilliseconds + serializationMilliseconds
        if total > 0 {
            activeDelayedOperations += 1
            defer { activeDelayedOperations -= 1 }
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

@MainActor
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
    sessionID: String,
    outbox: EventOutbox = EventOutbox()
) async throws -> SessionController {
    let controller = SessionController(
        transport: transport,
        outbox: outbox,
        renderer: renderer
    )
    controller.attachRenderer(renderer)
    try await controller.start()
    try await transport.injectFromServer(
        try framed(welcomeMessage(sessionID: sessionID))
    )
    let initial = Transaction(baseRevision: Revision(0), operations: fixtureOperations)
    try await transport.injectFromServer(
        try framed(transactionMessage(initial))
    )
    try await waitForRevision(Revision(1), controller: controller)
    try await waitUntil {
        renderer.registry.handle(for: NodeId(1)) != nil
            && renderer.registry.handle(for: NodeId(14)) != nil
            && renderer.registry.handle(for: NodeId(16)) != nil
    }
    guard controller.isEventDispatchEnabled else {
        throw BenchmarkFailure.message("benchmark session did not enable event dispatch")
    }
    return controller
}

@MainActor
private final class CadenceProbe {
    var repaintCount = 0
    var paintedRevision = Revision(1)
}

@MainActor
private func mutationAndCadence(
    fixtureOperations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) async throws -> Section {
    let baseApplier = TransactionApplier()
    guard case .success = baseApplier.apply(
        baseRevision: .initial,
        operations: fixtureOperations
    ) else {
        throw BenchmarkFailure.message("mutation benchmark base fixture did not commit")
    }
    let baseStore = baseApplier.store
    var metrics = [Metric]()
    var scalarOnly = true
    var allMutationRastersCompleted = true
    var allMutationStateParityPassed = true

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
        var messages = 0
        for _ in 0..<iterations {
            let result = try await mutationRun(
                baseStore: baseStore,
                fixtureOperations: fixtureOperations,
                operations: updates,
                fullPaint: fullPaint
            )
            semanticLatencies.append(result.semanticLatency)
            visibleLatencies.append(result.visibleLatency)
            bytes = result.bytes
            messages = result.messages
            allMutationRastersCompleted = allMutationRastersCompleted && result.rasterized
            allMutationStateParityPassed = allMutationStateParityPassed
                && result.stateParity
            scalarOnly = scalarOnly && result.classifications.allSatisfy {
                if case .structureAffecting = $0 { return false }
                return true
            }
        }
        let target = count == 100 ? 1.0 : (count == 1_000 ? 5.0 : nil)
        let semanticID = "updates.\(count).semantic"
        let visibleID = "updates.\(count).visible"
        metrics.append(metric("\(count) updates semantic decode/apply", p50(semanticLatencies), target: target, id: semanticID))
        metrics.append(metric("\(count) updates semantic decode/apply", percentile(semanticLatencies, 0.95), "ms", "p95", id: semanticID))
        metrics.append(metric("\(count) updates semantic decode/apply", percentile(semanticLatencies, 0.99), "ms", "p99", id: semanticID))
        metrics.append(metric("\(count) updates decode-to-visible", p50(visibleLatencies), id: visibleID))
        metrics.append(metric("\(count) updates decode-to-visible", percentile(visibleLatencies, 0.95), "ms", "p95", id: visibleID))
        metrics.append(metric("\(count) updates decode-to-visible", percentile(visibleLatencies, 0.99), "ms", "p99", id: visibleID))
        metrics.append(metric("\(count) updates wire bytes", Double(bytes), "bytes", "exact", id: "updates.\(count).bytes"))
        metrics.append(metric("\(count) updates message count", Double(messages), "messages", "exact", id: "updates.\(count).messages"))
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
        let sessionID = "cadence-session"
        let controller = try await startActiveSession(
            transport: transport,
            renderer: renderer,
            fixtureOperations: fixtureOperations,
            sessionID: sessionID
        )
        let beforeStream = await transport.snapshot()
        let probe = CadenceProbe()
        let cadenceTask = Task { @MainActor in
            let interval = Duration.milliseconds(1_000.0 / Double(hz))
            while Task.isCancelled == false {
                do {
                    try await Task.sleep(for: interval)
                } catch {
                    return
                }
                guard Task.isCancelled == false else { return }
                if rasterizeRenderer(
                    renderer,
                    showWindows: fullPaint && probe.repaintCount == 0
                ) {
                    probe.repaintCount += 1
                    probe.paintedRevision = controller.applier.lastAppliedRevision
                }
            }
        }

        var observedRevisions = [UInt64]()
        var expectedEvents = [CapturedEvent]()
        for index in 1...24 {
            try await Task.sleep(for: .milliseconds(4))
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
                try await transport.injectFromServer(
                    try framed(
                        acknowledgeMessage(
                            event,
                            outbox: controller.outbox,
                            sessionID: sessionID,
                            revision: revision
                        )
                    )
                )
                try await waitUntil {
                    await controller.outbox.pendingCount == 0
                }
            }
        }

        let finalRevision = Revision(25)
        try await Task.sleep(for: .milliseconds(25))
        cadenceTask.cancel()
        await cadenceTask.value
        if probe.paintedRevision != finalRevision,
           rasterizeRenderer(renderer, showWindows: fullPaint && probe.repaintCount == 0) {
            probe.repaintCount += 1
            probe.paintedRevision = finalRevision
        }

        let afterStream = await transport.snapshot()
        let streamBytes = (afterStream.inboundBytes - beforeStream.inboundBytes)
            + (afterStream.outboundBytes - beforeStream.outboundBytes)
        let streamMessages = (afterStream.inboundMessages - beforeStream.inboundMessages)
            + (afterStream.outboundMessages - beforeStream.outboundMessages)
        cadenceWire.append(streamBytes)
        cadenceMessages.append(streamMessages)
        repaintCounts.append(probe.repaintCount)
        metrics.append(metric("\(hz)Hz bidirectional wire bytes", Double(streamBytes), "bytes", "exact", id: "cadence.\(hz).bytes"))
        metrics.append(metric("\(hz)Hz bidirectional message count", Double(streamMessages), "messages", "exact", id: "cadence.\(hz).messages"))
        metrics.append(metric("\(hz)Hz live-clock repaint count", Double(probe.repaintCount), "repaints", "exact", id: "cadence.\(hz).repaints"))

        let progress = renderer.registry.view(for: NodeId(5)) as? NSProgressIndicator
        finalValues.append(progress?.doubleValue ?? -1)
        revisionsPreserved = revisionsPreserved
            && observedRevisions == Array(2...25).map(UInt64.init)
            && probe.paintedRevision == finalRevision
        let wireEvents = try await capturedEvents(in: transport)
        eventsPreserved = eventsPreserved
            && wireEvents.count == expectedEvents.count
            && zip(wireEvents, expectedEvents).allSatisfy {
                $0.id == $1.id
                    && $0.sequence == $1.sequence
                    && $0.observedRevision == $1.observedRevision
            }

        let idleStart = await transport.snapshot()
        try await Task.sleep(for: .milliseconds(20))
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

    metrics.append(metric("idle UI wire bytes", Double(idleBytes.max() ?? -1), "bytes", "observed max", id: "idle.bytes"))
    metrics.append(metric("idle UI message count", Double(idleMessages.max() ?? -1), "messages", "observed max", id: "idle.messages"))

    return Section(
        id: "31.3",
        name: "Mutation and frame independence",
        metrics: metrics,
        assertions: [
            Assertion(
                id: "mutation_raster_completion",
                name: fullPaint
                    ? "decode-to-visible samples cross the display framebuffer boundary"
                    : "decode-to-visible smoke samples complete the offscreen raster fallback",
                passed: allMutationRastersCompleted && allMutationStateParityPassed,
                detail: fullPaint
                    ? "every framed production SessionController sample matched the isolated TransactionApplier state and awaited an on-screen framebuffer advance"
                    : "every framed production SessionController smoke sample matched the isolated TransactionApplier state and produced an AppKit bitmap"
            ),
            Assertion(
                id: "idle_zero_traffic",
                name: "idle semantic UI emits zero observed SRUI traffic",
                passed: idleBytes.allSatisfy { $0 == 0 } && idleMessages.allSatisfy { $0 == 0 },
                detail: "20ms idle observation deltas: bytes \(idleBytes), messages \(idleMessages)"
            ),
            Assertion(
                id: "cadence_wire_invariant",
                name: "wire bytes and message count are cadence independent",
                passed: Set(cadenceWire).count == 1 && Set(cadenceMessages).count == 1,
                detail: "bidirectional bytes \(cadenceWire), messages \(cadenceMessages)"
            ),
            Assertion(
                id: "cadence_repaint_independent",
                name: "live local repaint count varies independently",
                passed: Set(repaintCounts).count > 1
                    && repaintCounts.contains { $0 < 24 },
                detail: "independent renderer-task repaint counts \(repaintCounts); at least one cadence coalesced the 24 mutations"
            ),
            Assertion(
                id: "cadence_state_event_order",
                name: "presentation preserves committed revisions, state, and event order",
                passed: Set(finalValues).count == 1
                    && finalValues.first == 1.0
                    && scalarOnly
                    && revisionsPreserved
                    && eventsPreserved,
                detail: "all cadences committed revisions 2...25, emitted ordered events, and rendered progress 1.0"
            ),
        ],
        notes: [
            "The updates.*.semantic trial times ProtocolDecoder plus TransactionApplier only. A byte-identical framed trial traverses BenchmarkTransport and SessionController into the pre-presented warm AppKitRenderer for updates.*.visible; final revision/value parity and captured frame bytes/messages are asserted.",
            "A live independent renderer task sleeps at 60/120/144/240Hz while a separate 4ms mutation source traverses production framing, SessionController, EventOutbox, decoding, semantic apply, and AppKitRenderer. Both inbound transactions/acks and outbound events are counted."
        ]
    )
}

@MainActor
private func benchmarkTrace(_ message: String) {
    guard ProcessInfo.processInfo.environment["SRUI_BENCHMARK_TRACE"] == "1" else {
        return
    }
    FileHandle.standardError.write(Data("\(message)\n".utf8))
}

@MainActor
private func benchmarkPhase(_ message: String) {
    guard ProcessInfo.processInfo.environment["SRUI_BENCHMARK_PHASES"] == "1" else {
        return
    }
    FileHandle.standardError.write(Data("\(message)\n".utf8))
}
@MainActor
private final class BenchmarkApplicationDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        benchmarkTrace("benchmark application termination request suppressed")
        return .terminateCancel
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}

private struct LocalInteractionResult {
    let samples: [String: [Double]]
    let stateChecksPassed: Bool
    let everyDelayedRTTOverlapped: Bool
    let productionCallbacks: Int
    let hoverMode: String
    let menuMode: String
}

private actor TransportFailureObservation {
    private var failures = [SessionFailure]()

    func record(_ failure: SessionFailure) {
        failures.append(failure)
    }

    func snapshot() -> (count: Int, transportEnded: Bool) {
        (
            failures.count,
            failures.contains {
                if case .transportEnded = $0 { return true }
                return false
            }
        )
    }
}

@MainActor
private func rendererTextView(_ renderer: AppKitRenderer) -> NSTextView? {
    guard let handle = renderer.registry.handle(for: NodeId(14)) else { return nil }
    if let view = handle.view as? NSTextView { return view }
    return (handle.view as? NSScrollView)?.documentView as? NSTextView
}

@MainActor
private func withSessionRTTInFlight(
    rttMilliseconds: Int,
    controller: SessionController,
    transport: BenchmarkTransport,
    body: @MainActor () async throws -> Void
) async throws -> Bool {
    let baseRevision = controller.applier.lastAppliedRevision
    let nextRevision = Revision(baseRevision.value + 1)
    let progress = Double(nextRevision.value % 100) / 100.0
    let response = Transaction(
        baseRevision: baseRevision,
        operations: [
            .setProperty(
                id: NodeId(5),
                property: .value,
                value: .float64(progress)
            )
        ]
    )
    let responseFrame = try framed(transactionMessage(response))
    benchmarkTrace("31.4 rtt=\(rttMilliseconds) overlap receive start")
    let receiveTask = Task {
        try await transport.injectFromServer(responseFrame)
    }
    if rttMilliseconds > 0 {
        try await waitUntil {
            await transport.snapshot().activeDelayedOperations > 0
        }
    } else {
        await Task.yield()
    }

    let delaySnapshot = await transport.snapshot()
    let delayWasActive = rttMilliseconds == 0
        || delaySnapshot.activeDelayedOperations > 0
    try await body()
    benchmarkTrace("31.4 rtt=\(rttMilliseconds) overlap body end")
    try await receiveTask.value
    try await waitForRevision(nextRevision, controller: controller)
    benchmarkTrace("31.4 rtt=\(rttMilliseconds) overlap receive end")
    return delayWasActive
}

@MainActor
private func benchmarkMouseEvent(
    _ type: NSEvent.EventType,
    window: NSWindow,
    location: NSPoint,
    eventNumber: Int
) throws -> NSEvent {
    guard let event = NSEvent.mouseEvent(
        with: type,
        location: location,
        modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: window.windowNumber,
        context: nil,
        eventNumber: eventNumber,
        clickCount: 1,
        pressure: type == .leftMouseDown ? 1 : 0
    ) else {
        throw BenchmarkFailure.message("could not construct AppKit mouse event")
    }
    return event
}

@MainActor
private func benchmarkTrackingEvent(
    _ type: NSEvent.EventType,
    window: NSWindow,
    location: NSPoint,
    eventNumber: Int
) throws -> NSEvent {
    guard let event = NSEvent.enterExitEvent(
        with: type,
        location: location,
        modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: window.windowNumber,
        context: nil,
        eventNumber: eventNumber,
        trackingNumber: 1,
        userData: nil
    ) else {
        throw BenchmarkFailure.message("could not construct AppKit tracking event")
    }
    return event
}

@MainActor
private func renderedBitmapSignature(_ view: NSView) -> Data? {
    view.layoutSubtreeIfNeeded()
    let rect = view.bounds.integral
    guard rect.width > 0, rect.height > 0,
          let representation = view.bitmapImageRepForCachingDisplay(in: rect) else {
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
private final class MenuTrackingProbe: NSObject, NSMenuDelegate {
    private(set) var openCount = 0
    private(set) var presentationCount = 0
    private(set) var observedAt: ContinuousClock.Instant?
    private var fullPaint = false
    private weak var host: NSView?
    private var screen: NSScreen?
    private var framebufferBaseline: TimeInterval = 0

    func prepare(fullPaint: Bool, host: NSView, screen: NSScreen?) {
        self.fullPaint = fullPaint
        self.host = host
        self.screen = screen
        framebufferBaseline = screen?.lastDisplayUpdateTimestamp ?? 0
        observedAt = nil
    }

    func menuWillOpen(_ menu: NSMenu) {
        openCount += 1
    }

    @objc func observePresentationAndCancel(_ menu: NSMenu) {
        let presented: Bool
        if fullPaint, let screen {
            presented = (try? benchmarkAwaitFramebufferAdvance(
                screen: screen,
                after: framebufferBaseline
            )) != nil
        } else if let host {
            presented = rasterize(host)
        } else {
            presented = false
        }
        if presented {
            presentationCount += 1
            observedAt = clock.now
        }
        menu.cancelTrackingWithoutAnimation()
    }
}
@MainActor
private func localInteractionSamples(
    renderer: AppKitRenderer,
    controller: SessionController,
    transport: BenchmarkTransport,
    sessionID: String,
    rttMilliseconds: Int,
    iterations: Int,
    fullPaint: Bool
) async throws -> LocalInteractionResult {
    let textHandle = renderer.registry.handle(for: NodeId(14))
    let textView = rendererTextView(renderer)
    let textScroll = (textHandle?.view as? NSScrollView) ?? textView?.enclosingScrollView
    let button = renderer.registry.view(for: NodeId(16)) as? NSButton
    let surface = renderer.registry.handle(for: NodeId(1))
    let window = surface?.window
    let host = window?.contentView
    let productionHandler = renderer.onInteraction
    guard let textView,
          let textScroll,
          let button,
          let window,
          let host,
          let productionHandler else {
        throw BenchmarkFailure.message(
            "representative native interaction controls did not mount: "
                + "text_handle=\(textHandle != nil), text_view=\(textView != nil), "
                + "text_scroll=\(textScroll != nil), button=\(button != nil), "
                + "surface=\(surface != nil), window=\(window != nil), "
                + "host=\(host != nil), production_handler=\(productionHandler != nil)"
        )
    }
    if fullPaint {
        window.makeKeyAndOrderFront(nil)
    }
    host.layoutSubtreeIfNeeded()
    textView.frame.size.height = max(textView.frame.height, 5_000)
    let previousMenu = button.menu
    let trackingArea = NSTrackingArea(
        rect: button.bounds,
        options: [.mouseEnteredAndExited, .activeAlways],
        owner: button,
        userInfo: nil
    )
    button.addTrackingArea(trackingArea)

    var callbackCount = 0
    renderer.onInteraction = { interaction in
        callbackCount += 1
        productionHandler(interaction)
    }
    defer {
        renderer.onInteraction = productionHandler
        button.removeTrackingArea(trackingArea)
        button.menu = previousMenu
    }

    var samples = [String: [Double]]()
    var checks = true
    var everyDelayedRTTOverlapped = true
    var menuOpened = 0
    var hoverVisualChanges = 0

    func record(
        _ id: String,
        action: () throws -> Bool,
        cleanup: () -> Bool = { true }
    ) async throws {
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) start")
        var sample = 0.0
        var samplePassed = false
        let overlap = try await withSessionRTTInFlight(
            rttMilliseconds: rttMilliseconds,
            controller: controller,
            transport: transport
        ) {
            let start = clock.now
            do {
                benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) action start")
                let stateCorrect = try action()
                benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) action end")
                let pixels: Bool
                if fullPaint {
                    pixels = try await benchmarkObserveOnScreenPaint(window)
                        .crossedDisplayRefresh
                } else {
                    pixels = rasterize(host)
                }
                let visibleAt = clock.now
                benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) paint end")
                let cleanupCorrect = cleanup()
                sample = milliseconds(start.duration(to: visibleAt))
                samplePassed = stateCorrect && pixels && cleanupCorrect
            } catch {
                _ = cleanup()
                throw error
            }
        }
        samples[id, default: []].append(sample)
        checks = checks && samplePassed
        everyDelayedRTTOverlapped = everyDelayedRTTOverlapped && overlap
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) end")
    }

    for _ in 0..<iterations {
        try await record("text_entry") {
            let before = (textView.string as NSString).length
            textView.insertText(
                "x",
                replacementRange: NSRange(location: before, length: 0)
            )
            return (textView.string as NSString).length == before + 1
        }
    }
    for index in 0..<iterations {
        try await record("caret_movement") {
            let location = min(index % 4, (textView.string as NSString).length)
            textView.setSelectedRange(NSRange(location: location, length: 0))
            return textView.selectedRange().location == location
                && textView.selectedRange().length == 0
        }
    }
    for _ in 0..<iterations {
        try await record("text_selection") {
            let length = min(2, (textView.string as NSString).length)
            textView.setSelectedRange(NSRange(location: 0, length: length))
            return textView.selectedRange().length == length
        }
    }
    for _ in 0..<iterations {
        try await record(
            "ime_composition",
            action: {
                textView.setMarkedText(
                    "é",
                    selectedRange: NSRange(location: 1, length: 0),
                    replacementRange: NSRange(location: NSNotFound, length: 0)
                )
                return textView.hasMarkedText()
            },
            cleanup: {
                textView.unmarkText()
                return textView.hasMarkedText() == false
            }
        )
    }
    for index in 0..<iterations {
        try await record("scrolling") {
            let target = NSPoint(x: 0, y: min(4_000, Double(index * 23)))
            textScroll.contentView.scroll(to: target)
            textScroll.reflectScrolledClipView(textScroll.contentView)
            return abs(textScroll.contentView.bounds.origin.y - target.y) < 1
        }
    }
    for index in 0..<iterations {
        let center = button.convert(
            NSPoint(x: button.bounds.midX, y: button.bounds.midY),
            to: nil
        )
        let hover = try benchmarkTrackingEvent(
            .mouseEntered,
            window: window,
            location: center,
            eventNumber: index * 4
        )
        let exit = try benchmarkTrackingEvent(
            .mouseExited,
            window: window,
            location: center,
            eventNumber: index * 4 + 1
        )
        let down = try benchmarkMouseEvent(
            .leftMouseDown,
            window: window,
            location: center,
            eventNumber: index * 4 + 2
        )
        let up = try benchmarkMouseEvent(
            .leftMouseUp,
            window: window,
            location: center,
            eventNumber: index * 4 + 3
        )
        let callbacksBefore = callbackCount
        try await record(
            "hover_pressed",
            action: {
                guard let beforeHover = renderedBitmapSignature(button) else {
                    return false
                }
                button.mouseEntered(with: hover)
                if let afterHover = renderedBitmapSignature(button),
                   afterHover != beforeHover {
                    hoverVisualChanges += 1
                }
                window.sendEvent(down)
                return button.isHighlighted
            },
            cleanup: {
                window.sendEvent(up)
                button.mouseExited(with: exit)
                return button.isHighlighted == false
                    && callbackCount == callbacksBefore + 1
            }
        )
    }
    for index in 0..<iterations {
        let menu = NSMenu(title: "Benchmark local menu")
        for title in ["One", "Two", "Three"] {
            menu.addItem(withTitle: title, action: nil, keyEquivalent: "")
        }
        let menuProbe = MenuTrackingProbe()
        menu.delegate = menuProbe
        button.menu = menu

        benchmarkTrace("31.4 rtt=\(rttMilliseconds) menu_opening start")
        var sample = 0.0
        var samplePassed = false
        let overlap = try await withSessionRTTInFlight(
            rttMilliseconds: rttMilliseconds,
            controller: controller,
            transport: transport
        ) {
            let start = clock.now
            let opensBefore = menuProbe.openCount
            let presentationsBefore = menuProbe.presentationCount
            menuProbe.prepare(
                fullPaint: fullPaint,
                host: host,
                screen: window.screen ?? NSScreen.main
            )
            let observeSelector = #selector(
                MenuTrackingProbe.observePresentationAndCancel(_:)
            )
            RunLoop.main.perform(
                observeSelector,
                target: menuProbe,
                argument: menu,
                order: 0,
                modes: [.eventTracking]
            )
            _ = menu.popUp(
                positioning: menu.items.first,
                at: NSPoint(x: button.bounds.minX, y: button.bounds.maxY),
                in: button
            )
            menuOpened += 1
            if let observedAt = menuProbe.observedAt {
                sample = milliseconds(start.duration(to: observedAt))
            }
            samplePassed = menuOpened == index + 1
                && menuProbe.openCount == opensBefore + 1
                && menuProbe.presentationCount == presentationsBefore + 1
                && menuProbe.observedAt != nil
        }
        samples["menu_opening", default: []].append(sample)
        checks = checks && samplePassed
        everyDelayedRTTOverlapped = everyDelayedRTTOverlapped && overlap
        button.menu = previousMenu
        menu.delegate = nil
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) menu_opening end")
    }

    try await waitUntil(timeout: .seconds(10)) {
        await transport.snapshot().activeDelayedOperations == 0
    }
    return LocalInteractionResult(
        samples: samples,
        stateChecksPassed: checks,
        everyDelayedRTTOverlapped: everyDelayedRTTOverlapped,
        productionCallbacks: callbackCount,
        hoverMode: hoverVisualChanges > 0
            ? "renderer-produced NSButton changed raster on its AppKit hover path in \(hoverVisualChanges)/\(iterations) samples"
            : "renderer-produced NSButton handled AppKit hover entry/exit but exposed no distinct hover raster; no visual-hover claim is made",
        menuMode: "renderer-produced NSButton context NSMenu opened and deterministically cancelled through AppKit event tracking"
    )
}
private func acknowledgementForCapturedEvent(
    _ event: CapturedEvent,
    outbox: EventOutbox,
    sessionID: String,
    revision: Revision
) -> SRUIMessage {
    var acknowledgement = SRUIServerEventAck()
    acknowledgement.sessionID = sessionID
    acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
    acknowledgement.eventID = event.id
    acknowledgement.lastProcessedEventSeq = event.sequence
    acknowledgement.status = .processed
    acknowledgement.revisionAfterEffect = revision.value
    var message = SRUIMessage()
    message.serverEventAck = acknowledgement
    return message
}

@MainActor
private func networkAndLocalInteraction(
    fixtureOperations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) async throws -> Section {
    let frameBudget = benchmarkLocalFrameBudget(fullPaint: fullPaint)
    var metrics = [
        metric(
            "local display frame budget",
            frameBudget.milliseconds,
            "ms",
            "exact",
            id: "display.frame_budget"
        )
    ]
    var localByRTT = [Int: [String: [Double]]]()
    var dependentByRTT = [Int: [Double]]()
    var allLocalStateChecks = true
    var allDelayedRTTOverlapped = true
    var productionCallbackCount = 0
    var hoverModes = Set<String>()
    var menuModes = Set<String>()
    var measuredWireBytes = 0
    var measuredWireMessages = 0

    for rtt in [0, 100, 300, 600] {
        benchmarkTrace("31.4 rtt=\(rtt) begin")
        let transport = BenchmarkTransport(rttMilliseconds: rtt)
        let renderer = AppKitRenderer()
        let sessionID = "network-\(rtt)"
        let controller = try await startActiveSession(
            transport: transport,
            renderer: renderer,
            fixtureOperations: fixtureOperations,
            sessionID: sessionID
        )

        let local = try await localInteractionSamples(
            renderer: renderer,
            controller: controller,
            transport: transport,
            sessionID: sessionID,
            rttMilliseconds: rtt,
            iterations: iterations,
            fullPaint: fullPaint
        )
        benchmarkPhase("31.4 rtt=\(rtt) local interactions finished")
        localByRTT[rtt] = local.samples
        allLocalStateChecks = allLocalStateChecks && local.stateChecksPassed
        allDelayedRTTOverlapped = allDelayedRTTOverlapped
            && local.everyDelayedRTTOverlapped
        productionCallbackCount += local.productionCallbacks
        hoverModes.insert(local.hoverMode)
        menuModes.insert(local.menuMode)

        renderer.textEditingSession.flushAllPending()
        try await Task.sleep(for: .milliseconds(10))
        try await waitUntil(timeout: .seconds(20)) {
            let pending = await controller.outbox.pendingCount
            let captured = try? await capturedEvents(in: transport).count
            let delayed = await transport.snapshot().activeDelayedOperations
            return pending > 0 && captured == pending && delayed == 0
        }
        let interactionEvents = try await capturedEvents(in: transport)
        benchmarkTrace(
            "31.4 rtt=\(rtt) acknowledging \(interactionEvents.count) renderer events"
        )
        for event in interactionEvents {
            try await transport.injectFromServer(
                try framed(
                    acknowledgementForCapturedEvent(
                        event,
                        outbox: controller.outbox,
                        sessionID: sessionID,
                        revision: controller.applier.lastAppliedRevision
                    )
                )
            )
        }
        try await waitUntil(timeout: .seconds(20)) {
            await controller.outbox.pendingCount == 0
        }
        benchmarkPhase("31.4 rtt=\(rtt) renderer events acknowledged")

        for (kind, values) in local.samples.sorted(by: { $0.key < $1.key }) {
            let id = "interaction.\(kind).rtt.\(rtt)"
            let displayName = kind.replacingOccurrences(of: "_", with: " ")
            metrics.append(metric("\(displayName) at \(rtt)ms RTT", p50(values), target: frameBudget.milliseconds, id: id))
            metrics.append(metric("\(displayName) at \(rtt)ms RTT", percentile(values, 0.95), "ms", "p95", id: id))
            metrics.append(metric("\(displayName) at \(rtt)ms RTT", percentile(values, 0.99), "ms", "p99", id: id))
        }

        let localTransportStats = await transport.snapshot()
        renderer.onInteraction = nil
        await controller.stop()
        closeRenderer(renderer)

        let feedbackTransport = BenchmarkTransport(rttMilliseconds: rtt)
        let feedbackRenderer = AppKitRenderer()
        let feedbackSessionID = "network-feedback-\(rtt)"
        let feedbackController = try await startActiveSession(
            transport: feedbackTransport,
            renderer: feedbackRenderer,
            fixtureOperations: fixtureOperations,
            sessionID: feedbackSessionID
        )
        let feedbackWarmVisible: Bool
        if fullPaint {
            guard let window = feedbackRenderer.registry.surfaceHandles.first?.window else {
                throw BenchmarkFailure.message("network feedback has no warm presentation window")
            }
            feedbackWarmVisible = try await benchmarkObserveOnScreenPaint(window)
                .crossedDisplayRefresh
        } else {
            feedbackWarmVisible = rasterizeRenderer(feedbackRenderer, showWindows: false)
        }
        guard feedbackWarmVisible else {
            throw BenchmarkFailure.message("network feedback renderer did not warm")
        }

        var dependent = [Double]()
        let responseIterations = max(3, min(7, iterations))
        for sample in 0..<responseIterations {
            benchmarkTrace("31.4 rtt=\(rtt) server feedback \(sample) start")
            let baseRevision = feedbackController.applier.lastAppliedRevision
            let expectedValue = Double(sample + 1) / Double(responseIterations)
            let eventsBefore = try await capturedEvents(in: feedbackTransport).count
            let start = clock.now
            let event = try await feedbackController.sendValueChanged(
                nodeId: NodeId(5),
                value: .float64(expectedValue)
            )
            let eventsAfter = try await capturedEvents(in: feedbackTransport)
            guard eventsAfter.count == eventsBefore + 1,
                  let capturedEvent = eventsAfter.last,
                  capturedEvent.id == event.eventId.bytes,
                  capturedEvent.sequence == event.eventSeq,
                  capturedEvent.observedRevision == baseRevision.value else {
                throw BenchmarkFailure.message(
                    "server-feedback trial did not emit exactly one matching framed production event"
                )
            }
            let response = Transaction(
                baseRevision: baseRevision,
                operations: [
                    .setProperty(
                        id: NodeId(5),
                        property: .value,
                        value: .float64(expectedValue)
                    )
                ]
            )
            try await feedbackTransport.injectFromServer(
                try framed(transactionMessage(response))
            )
            let nextRevision = Revision(baseRevision.value + 1)
            try await waitForRevision(nextRevision, controller: feedbackController)
            try await waitUntil {
                guard let progress = feedbackRenderer.registry.view(for: NodeId(5))
                    as? NSProgressIndicator else {
                    return false
                }
                return abs(progress.doubleValue - expectedValue) < 0.000_001
            }
            let feedbackVisible: Bool
            if fullPaint {
                guard let window = feedbackRenderer.registry.surfaceHandles.first?.window else {
                    throw BenchmarkFailure.message("network response has no presentation window")
                }
                feedbackVisible = try await benchmarkObserveOnScreenPaint(window)
                    .crossedDisplayRefresh
            } else {
                feedbackVisible = rasterizeRenderer(feedbackRenderer, showWindows: false)
            }
            guard feedbackVisible else {
                throw BenchmarkFailure.message("network response did not reach the measured visible boundary")
            }
            dependent.append(milliseconds(start.duration(to: clock.now)))
            try await feedbackTransport.injectFromServer(
                try framed(
                    acknowledgeMessage(
                        event,
                        outbox: feedbackController.outbox,
                        sessionID: feedbackSessionID,
                        revision: nextRevision
                    )
                )
            )
            try await waitUntil(timeout: .seconds(20)) {
                await feedbackController.outbox.pendingCount == 0
            }
            benchmarkTrace("31.4 rtt=\(rtt) server feedback \(sample) end")
        }
        benchmarkPhase("31.4 rtt=\(rtt) server feedback finished")
        dependentByRTT[rtt] = dependent
        let feedbackID = "server_feedback.rtt.\(rtt)"
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", p50(dependent), id: feedbackID))
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", percentile(dependent, 0.95), "ms", "p95", id: feedbackID))
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", percentile(dependent, 0.99), "ms", "p99", id: feedbackID))

        let feedbackTransportStats = await feedbackTransport.snapshot()
        measuredWireBytes += localTransportStats.outboundBytes
            + localTransportStats.inboundBytes
            + feedbackTransportStats.outboundBytes
            + feedbackTransportStats.inboundBytes
        measuredWireMessages += localTransportStats.outboundMessages
            + localTransportStats.inboundMessages
            + feedbackTransportStats.outboundMessages
            + feedbackTransportStats.inboundMessages
        await feedbackController.stop()
        closeRenderer(feedbackRenderer)
        benchmarkTrace("31.4 rtt=\(rtt) complete")
        benchmarkPhase("31.4 rtt=\(rtt) complete")
    }

    benchmarkTrace("31.4 impairments begin")
    benchmarkPhase("31.4 impairments begin")
    let bandwidthTransport = BenchmarkTransport(bytesPerSecond: 1_048_576)
    let bandwidthRenderer = AppKitRenderer()
    let bandwidthController = try await startActiveSession(
        transport: bandwidthTransport,
        renderer: bandwidthRenderer,
        fixtureOperations: fixtureOperations,
        sessionID: "bandwidth"
    )
    let bandwidthBefore = await bandwidthTransport.snapshot()
    let largeValue = SemanticModel.Value.string(String(repeating: "x", count: 16_384))
    var bandwidthSamples = [Double]()
    for _ in 0..<3 {
        let start = clock.now
        let event = try await bandwidthController.sendValueChanged(
            nodeId: NodeId(16),
            value: largeValue
        )
        bandwidthSamples.append(milliseconds(start.duration(to: clock.now)))
        try await bandwidthTransport.injectFromServer(
            try framed(
                acknowledgeMessage(
                    event,
                    outbox: bandwidthController.outbox,
                    sessionID: "bandwidth",
                    revision: bandwidthController.applier.lastAppliedRevision
                )
            )
        )
    }
    let bandwidthAfter = await bandwidthTransport.snapshot()
    let bandwidthDeliveredBytes = bandwidthAfter.outboundBytes - bandwidthBefore.outboundBytes
    measuredWireBytes += bandwidthDeliveredBytes
    measuredWireMessages += bandwidthAfter.outboundMessages - bandwidthBefore.outboundMessages
    await bandwidthController.stop()
    closeRenderer(bandwidthRenderer)
    metrics.append(metric("1MiB/s bandwidth-limited production event", p50(bandwidthSamples), id: "impairment.bandwidth_transfer"))
    metrics.append(metric("1MiB/s bandwidth-limited production event", percentile(bandwidthSamples, 0.95), "ms", "p95", id: "impairment.bandwidth_transfer"))
    metrics.append(metric("bandwidth-limited delivered bytes", Double(bandwidthDeliveredBytes), "bytes", "exact", id: "impairment.bandwidth_delivered_bytes"))

    let lossOutbox = EventOutbox()
    let lossTransport = BenchmarkTransport(dropOutboundOrdinals: [2])
    let lossRenderer = AppKitRenderer()
    let lossController = try await startActiveSession(
        transport: lossTransport,
        renderer: lossRenderer,
        fixtureOperations: fixtureOperations,
        sessionID: "loss",
        outbox: lossOutbox
    )
    let lossBefore = await lossTransport.snapshot()
    let lostEvent = try await lossController.sendActivate(nodeId: NodeId(16))
    let lossAfter = await lossTransport.snapshot()
    let retainedAfterLoss = await lossOutbox.pendingCount == 1
    await lossController.stop()
    closeRenderer(lossRenderer)

    let lossRecoveryTransport = BenchmarkTransport()
    let lossRecoveryController = SessionController(
        transport: lossRecoveryTransport,
        outbox: lossOutbox,
        sessionId: "loss"
    )
    try await lossRecoveryController.start()
    let lossRecoveryBefore = await lossRecoveryTransport.snapshot()
    try await lossRecoveryTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "loss"))
    )
    try await waitUntil {
        (try? await capturedEvents(in: lossRecoveryTransport).count) == 1
    }
    let recoveredLossEvents = try await capturedEvents(in: lossRecoveryTransport)
    let replayedLostEvent = recoveredLossEvents.first?.id == lostEvent.eventId.bytes
    try await lossRecoveryTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                recoveredLossEvents[0],
                outbox: lossOutbox,
                sessionID: "loss",
                revision: lossRecoveryController.applier.lastAppliedRevision
            )
        )
    )
    try await waitUntil {
        await lossOutbox.pendingCount == 0
    }
    let lossRecoveryAfter = await lossRecoveryTransport.snapshot()
    await lossRecoveryController.stop()
    let lossAttempts = (lossAfter.outboundAttempts - lossBefore.outboundAttempts)
        + (lossRecoveryAfter.outboundAttempts - lossRecoveryBefore.outboundAttempts)
    let lossDeliveredMessages = (lossAfter.outboundMessages - lossBefore.outboundMessages)
        + (lossRecoveryAfter.outboundMessages - lossRecoveryBefore.outboundMessages)
    metrics.append(metric("deterministic production loss attempts", Double(lossAttempts), "messages", "exact", id: "impairment.loss_attempts"))
    metrics.append(metric("deterministic production loss delivered messages", Double(lossDeliveredMessages), "messages", "exact", id: "impairment.loss_delivered_messages"))

    let interruptionOutbox = EventOutbox()
    let interruptionTransport = BenchmarkTransport(interruptOutboundOrdinals: [2])
    let interruptionRenderer = AppKitRenderer()
    let interruptionController = try await startActiveSession(
        transport: interruptionTransport,
        renderer: interruptionRenderer,
        fixtureOperations: fixtureOperations,
        sessionID: "interruption",
        outbox: interruptionOutbox
    )
    let interruptionFailures = TransportFailureObservation()
    interruptionController.onFailure = { failure in
        Task {
            await interruptionFailures.record(failure)
        }
    }
    let interruptionStart = clock.now
    var interruptionFailed = false
    do {
        _ = try await interruptionController.sendActivate(nodeId: NodeId(16))
    } catch TransportError.closed {
        interruptionFailed = true
    }
    try await waitUntil(timeout: .seconds(10)) {
        let observed = await interruptionFailures.snapshot()
        let stats = await interruptionTransport.snapshot()
        return interruptionController.isDiverged
            && observed.count == 1
            && observed.transportEnded
            && stats.isClosed
            && stats.closeCalls >= 1
    }
    let interruptionLatency = milliseconds(interruptionStart.duration(to: clock.now))
    let interruptionPending = await interruptionOutbox.pendingCount == 1
    let interruptionFailureSnapshot = await interruptionFailures.snapshot()
    let interruptionStats = await interruptionTransport.snapshot()
    let interruptionLifecycleObserved = interruptionController.isDiverged
        && interruptionFailureSnapshot.count == 1
        && interruptionFailureSnapshot.transportEnded
        && interruptionStats.isClosed
        && interruptionStats.closeCalls >= 1
    await interruptionController.stop()
    closeRenderer(interruptionRenderer)

    let interruptionRecoveryTransport = BenchmarkTransport()
    let interruptionRecoveryController = SessionController(
        transport: interruptionRecoveryTransport,
        outbox: interruptionOutbox,
        sessionId: "interruption"
    )
    try await interruptionRecoveryController.start()
    let interruptionRecoveryBefore = await interruptionRecoveryTransport.snapshot()
    try await interruptionRecoveryTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "interruption"))
    )
    try await waitUntil {
        (try? await capturedEvents(in: interruptionRecoveryTransport).count) == 1
    }
    let recoveredInterruptionEvents = try await capturedEvents(
        in: interruptionRecoveryTransport
    )
    try await interruptionRecoveryTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                recoveredInterruptionEvents[0],
                outbox: interruptionOutbox,
                sessionID: "interruption",
                revision: interruptionRecoveryController.applier.lastAppliedRevision
            )
        )
    )
    try await waitUntil {
        await interruptionOutbox.pendingCount == 0
    }
    let interruptionRecoveryAfter = await interruptionRecoveryTransport.snapshot()
    await interruptionRecoveryController.stop()
    metrics.append(metric("controlled production interruption detection", interruptionLatency, id: "impairment.interruption_detection"))

    measuredWireBytes += (lossAfter.outboundBytes - lossBefore.outboundBytes)
        + (lossRecoveryAfter.outboundBytes - lossRecoveryBefore.outboundBytes)
        + (lossRecoveryAfter.inboundBytes - lossRecoveryBefore.inboundBytes)
        + interruptionStats.outboundBytes + interruptionStats.inboundBytes
        + (interruptionRecoveryAfter.outboundBytes - interruptionRecoveryBefore.outboundBytes)
        + (interruptionRecoveryAfter.inboundBytes - interruptionRecoveryBefore.inboundBytes)
    measuredWireMessages += (lossAfter.outboundMessages - lossBefore.outboundMessages)
        + (lossRecoveryAfter.outboundMessages - lossRecoveryBefore.outboundMessages)
        + (lossRecoveryAfter.inboundMessages - lossRecoveryBefore.inboundMessages)
        + interruptionStats.outboundMessages + interruptionStats.inboundMessages
        + (interruptionRecoveryAfter.outboundMessages - interruptionRecoveryBefore.outboundMessages)
        + (interruptionRecoveryAfter.inboundMessages - interruptionRecoveryBefore.inboundMessages)
    metrics.append(metric("measured production session wire bytes", Double(measuredWireBytes), "bytes", "exact", id: "session_wire.bytes"))
    metrics.append(metric("measured production session wire messages", Double(measuredWireMessages), "messages", "exact", id: "session_wire.messages"))

    let baseline = localByRTT[0] ?? [:]
    var p50Added = [Double]()
    var p95Added = [Double]()
    var p99Added = [Double]()
    for (rtt, samples) in localByRTT where rtt != 0 {
        for (kind, values) in samples {
            guard let base = baseline[kind] else { continue }
            p50Added.append(p50(values) - p50(base))
            p95Added.append(percentile(values, 0.95) - percentile(base, 0.95))
            p99Added.append(percentile(values, 0.99) - percentile(base, 0.99))
        }
    }
    let worstP50Added = max(0, p50Added.max() ?? .infinity)
    let worstP95Added = max(0, p95Added.max() ?? .infinity)
    let worstP99Added = max(0, p99Added.max() ?? .infinity)
    let minimumInjectedOneWayDelayMilliseconds = 50.0
    metrics.append(metric("maximum RTT-induced local latency delta", worstP50Added, "ms", "p50", target: frameBudget.milliseconds, id: "local_rtt_delta"))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP95Added, "ms", "p95", id: "local_rtt_delta"))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP99Added, "ms", "p99", id: "local_rtt_delta"))

    let serverTracksRTT = [100, 300, 600].allSatisfy {
        guard let samples = dependentByRTT[$0] else { return false }
        return p50(samples) >= Double($0) * 0.80
    }
    let lossPendingCleared = await lossOutbox.pendingCount == 0
    let interruptionPendingCleared = await interruptionOutbox.pendingCount == 0
    let impairmentsApplied = bandwidthDeliveredBytes > 16_384 * 3
        && lossAttempts == 2
        && lossDeliveredMessages == 1
        && lossAfter.droppedMessages == 1
        && retainedAfterLoss
        && replayedLostEvent
        && interruptionFailed
        && interruptionPending
        && interruptionStats.interruptions == 1
        && interruptionLifecycleObserved
        && recoveredInterruptionEvents.count == 1
        && lossPendingCleared
        && interruptionPendingCleared

    benchmarkTrace("31.4 complete")
    benchmarkPhase("31.4 complete")
    return Section(
        id: "31.4",
        name: "Network and local interaction",
        metrics: metrics,
        assertions: [
            Assertion(
                id: "local_latency_independent",
                name: "mounted local interactions do not acquire one RTT",
                passed: worstP95Added < minimumInjectedOneWayDelayMilliseconds
                    && allLocalStateChecks
                    && allDelayedRTTOverlapped
                    && productionCallbackCount >= iterations * 4,
                detail: "largest p95 increase \(String(format: "%.4f", worstP95Added)) ms, below the minimum injected one-way delay of \(String(format: "%.1f", minimumInjectedOneWayDelayMilliseconds)) ms; all delayed RTT probes overlapped=\(allDelayedRTTOverlapped); production renderer callbacks=\(productionCallbackCount)"
            ),
            Assertion(
                id: "server_latency_tracks_rtt",
                name: "injected transport RTT affects production server-dependent feedback",
                passed: serverTracksRTT,
                detail: "SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT"
            ),
            Assertion(
                id: "no_sync_rtt",
                name: "render and local-feedback paths perform no synchronous network RTT",
                passed: worstP99Added < minimumInjectedOneWayDelayMilliseconds,
                detail: "largest p99 local delta was \(String(format: "%.4f", worstP99Added)) ms, below the minimum injected one-way delay of \(String(format: "%.1f", minimumInjectedOneWayDelayMilliseconds)) ms while production sends were measurably delayed"
            ),
            Assertion(
                id: "impairments_use_session",
                name: "bandwidth, loss, and interruption exercise session recovery",
                passed: impairmentsApplied,
                detail: "\(bandwidthDeliveredBytes) bandwidth bytes; lost and interrupted events remained in EventOutbox and replayed through replacement SessionControllers"
            ),
        ],
        notes: [
            "Controls are mounted renderer TextArea, ScrollView, and Button. \(menuModes.sorted().joined(separator: "; ")).",
            "Pressed state is observed between real NSWindow-dispatched mouseDown/mouseUp events and triggers the production ActionTrampoline. \(hoverModes.sorted().joined(separator: "; ")). Permission-free hover invokes the renderer-produced NSButton's own AppKit entry/exit path and does not claim WindowServer pointer latency.",
            "Local frame budget \(String(format: "%.6f", frameBudget.milliseconds)) ms came from \(frameBudget.source).",
            "All impairment traffic traverses SessionController, EventOutbox, SRUIFraming, and replacement-session resume/replay; no benchmark calls Transport.send directly.",
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

private actor ResumeFailureObservation {
    private var failures = [SessionFailure]()

    func record(_ failure: SessionFailure) {
        failures.append(failure)
    }

    func snapshot() -> (count: Int, containsSuperseded: Bool, descriptions: [String]) {
        (
            failures.count,
            failures.contains {
                if case .superseded = $0 { return true }
                return false
            },
            failures.map(\.description)
        )
    }
}

@MainActor
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
    try await firstTransport.injectFromServer(
        try framed(welcomeMessage(sessionID: "resource-session"))
    )
    try await waitUntil { first.isEventDispatchEnabled }
    let midpoint = fixturePNG.count / 2
    try await firstTransport.injectFromServer(
        try framed(resourceMetadataMessage(hash: hash))
    )
    try await firstTransport.injectFromServer(
        try framed(
            resourceChunkMessage(
                hash: hash,
                offset: 0,
                data: fixturePNG.prefix(midpoint)
            )
        )
    )
    try await waitUntil { await cache.retainedBytes() > 0 }
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
    try await secondTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "resource-session"))
    )
    try await waitUntil { second.isEventDispatchEnabled }
    try await secondTransport.injectFromServer(
        try framed(resourceMetadataMessage(hash: hash))
    )
    try await secondTransport.injectFromServer(
        try framed(
            resourceChunkMessage(
                hash: hash,
                offset: 0,
                data: fixturePNG.prefix(midpoint)
            )
        )
    )
    try await secondTransport.injectFromServer(
        try framed(
            resourceChunkMessage(
                hash: hash,
                offset: midpoint,
                data: fixturePNG.suffix(from: midpoint)
            )
        )
    )
    try await waitUntil { await cache.contains(resourceHash) }
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

@MainActor
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
    try await seedTransport.injectFromServer(
        try framed(welcomeMessage(sessionID: "superseded-session"))
    )
    try await waitUntil { seed.isEventDispatchEnabled }
    let pending = try await seed.sendActivate(nodeId: NodeId(16))
    await seed.stop()

    let oldFailures = ResumeFailureObservation()
    let oldTransport = BenchmarkTransport()
    let oldApplier = TransactionApplier()
    let oldController = SessionController(
        transport: oldTransport,
        applier: oldApplier,
        outbox: outbox,
        sessionId: "superseded-session"
    )
    oldController.onFailure = { failure in
        Task { await oldFailures.record(failure) }
    }
    try await oldController.start()

    let newFailures = ResumeFailureObservation()
    let newTransport = BenchmarkTransport()
    let newApplier = TransactionApplier()
    let newController = SessionController(
        transport: newTransport,
        applier: newApplier,
        outbox: outbox,
        sessionId: "superseded-session"
    )
    newController.onFailure = { failure in
        Task { await newFailures.record(failure) }
    }
    try await newController.start()

    let newWireBeforeOldResponse = await newTransport.snapshot()
    let pendingBeforeOldResponse = await outbox.pendingCount
    let newRevisionBeforeOldResponse = newApplier.lastAppliedRevision
    let oldStart = clock.now
    try await oldTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "superseded-session"))
    )
    try await waitUntil {
        let failure = await oldFailures.snapshot()
        let transport = await oldTransport.snapshot()
        return failure.count == 1 && transport.isClosed
    }
    let oldLatency = milliseconds(oldStart.duration(to: clock.now))

    let oldWireAfterResponse = await oldTransport.snapshot()
    let oldEvents = try await capturedEvents(in: oldTransport)
    let oldFailure = await oldFailures.snapshot()
    let oldDivergedAfterResponse = oldController.isDiverged
    let oldHandshakeAfterResponse = oldController.isHandshakeComplete
    let oldDispatchAfterResponse = oldController.isEventDispatchEnabled
    let newFailureAfterOldResponse = await newFailures.snapshot()
    let newWireAfterOldResponse = await newTransport.snapshot()
    let pendingAfterOldResponse = await outbox.pendingCount
    let newRevisionAfterOldResponse = newApplier.lastAppliedRevision

    let newStart = clock.now
    try await newTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "superseded-session"))
    )
    try await waitUntil {
        let events = try? await capturedEvents(in: newTransport)
        return events?.count == 1 && newController.isEventDispatchEnabled
    }
    let newLatency = milliseconds(newStart.duration(to: clock.now))
    var newEvents = try await capturedEvents(in: newTransport)
    let replayMatches = newEvents.first.map {
        $0.id == pending.eventId.bytes && $0.sequence == pending.eventSeq
    } ?? false
    try await newTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                newEvents[0],
                outbox: outbox,
                sessionID: "superseded-session",
                revision: newApplier.lastAppliedRevision
            )
        )
    )
    try await waitUntil { await outbox.pendingCount == 0 }

    let fresh = try await newController.sendActivate(nodeId: NodeId(16))
    try await waitUntil {
        (try? await capturedEvents(in: newTransport).count) == 2
    }
    newEvents = try await capturedEvents(in: newTransport)
    let freshActionMatches = newEvents.last.map {
        $0.id == fresh.eventId.bytes
            && $0.sequence == fresh.eventSeq
            && $0.observedRevision == fresh.observedRevision.value
    } ?? false
    try await newTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                newEvents[1],
                outbox: outbox,
                sessionID: "superseded-session",
                revision: newApplier.lastAppliedRevision
            )
        )
    )
    try await waitUntil { await outbox.pendingCount == 0 }

    let activeWireBeforeOldStop = await newTransport.snapshot()
    await oldController.stop()
    let activeWireAfterOldStop = await newTransport.snapshot()
    let newFailureAtEnd = await newFailures.snapshot()
    let newLifecycleUnaffected = newController.isDiverged == false
        && newController.isEventDispatchEnabled
        && newFailureAtEnd.count == 0
        && newApplier.lastAppliedRevision == .initial
        && newApplier.store.nodeCount == 0
        && activeWireAfterOldStop.outboundBytes == activeWireBeforeOldStop.outboundBytes
        && activeWireAfterOldStop.outboundMessages == activeWireBeforeOldStop.outboundMessages
        && activeWireAfterOldStop.inboundBytes == activeWireBeforeOldStop.inboundBytes
        && activeWireAfterOldStop.inboundMessages == activeWireBeforeOldStop.inboundMessages

    let checks = [
        "old_response_sent_only_to_old_transport":
            oldWireAfterResponse.inboundMessages == 1,
        "old_replay_is_empty": oldEvents.isEmpty,
        "old_lifecycle_reports_superseded":
            oldFailure.count == 1 && oldFailure.containsSuperseded,
        "old_lifecycle_diverged": oldDivergedAfterResponse,
        "old_lifecycle_handshake_failed": oldHandshakeAfterResponse == false,
        "old_lifecycle_dispatch_blocked": oldDispatchAfterResponse == false,
        "old_lifecycle_closed": oldWireAfterResponse.closeCalls == 1
            && oldWireAfterResponse.isClosed,
        "pending_unchanged_by_old_response":
            pendingBeforeOldResponse == 1 && pendingAfterOldResponse == 1,
        "new_wire_unchanged_by_old_response":
            newWireAfterOldResponse.outboundBytes == newWireBeforeOldResponse.outboundBytes
                && newWireAfterOldResponse.outboundMessages
                    == newWireBeforeOldResponse.outboundMessages
                && newWireAfterOldResponse.inboundBytes == newWireBeforeOldResponse.inboundBytes
                && newWireAfterOldResponse.inboundMessages
                    == newWireBeforeOldResponse.inboundMessages,
        "new_semantic_state_unchanged_by_old_response":
            newRevisionAfterOldResponse == newRevisionBeforeOldResponse
                && newApplier.store.nodeCount == 0,
        "new_failure_not_called_by_old_response": newFailureAfterOldResponse.count == 0,
        "new_response_replays_original_event": replayMatches,
        "new_action_identity_and_order_preserved": freshActionMatches,
        "new_lifecycle_unaffected_by_old_stop": newLifecycleUnaffected,
    ]
    let passed = checks.values.allSatisfy { $0 }
    let detail = checks.keys.sorted().map {
        "\($0)=\(checks[$0] == true)"
    }.joined(separator: ", ")
        + "; old_failures=\(oldFailure.descriptions)"

    await newController.stop()
    return (oldLatency, newLatency, passed, detail)
}

@MainActor
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
            metric("mid-resource reconnect recovery", p50(resourceLatencies), id: "mid_resource_recovery"),
            metric("mid-resource reconnect recovery", percentile(resourceLatencies, 0.95), "ms", "p95", id: "mid_resource_recovery"),
            metric("superseded resume response handling", p50(supersededLatencies), id: "superseded_response"),
            metric("superseded resume response handling", percentile(supersededLatencies, 0.95), "ms", "p95", id: "superseded_response"),
            metric("active resume response handling", p50(activeLatencies), id: "active_response"),
            metric("active resume response handling", percentile(activeLatencies, 0.95), "ms", "p95", id: "active_response"),
        ],
        assertions: [
            Assertion(
                id: "mid_resource_recovery",
                name: "mid-resource disconnect discards partial bytes and retransmission commits",
                passed: resourcePassed,
                detail: "SessionController framing and ownership retired invisible partials; replacement replayed metadata and contiguous chunks from offset zero"
            ),
            Assertion(
                id: "superseded_response_inert",
                name: "superseded response affects only the expected old lifecycle teardown",
                passed: supersededPassed,
                detail: supersededDetails.sorted().joined(separator: "; ")
            ),
        ],
        notes: [
            "The old resume response is inert with respect to active/new wire, event/action identity, semantic state, and lifecycle. Task 23 intentionally reports .superseded, marks the old controller diverged, and closes only its transport; those expected old-lifecycle effects are measured explicitly."
        ]
    )
}

private func terminalFullPlainText(_ snapshot: TerminalSnapshot) -> String {
    (snapshot.scrollback + snapshot.cells)
        .map { row in
            var characters = row.map(\.character)
            while let last = characters.last, last.isWhitespace {
                characters.removeLast()
            }
            return String(characters)
        }
        .joined(separator: "\n")
        .trimmingCharacters(in: CharacterSet.newlines.union(.whitespaces))
}

@MainActor
private func terminal(iterations: Int, fullPaint: Bool) async throws -> Section {
    let line = Data("\u{1b}[32mbenchmark output\u{1b}[0m\r\n".utf8)
    var payload = Data()
    payload.reserveCapacity(line.count * 256)
    for _ in 0..<256 { payload.append(line) }
    let expectedPlainText = Array(
        repeating: "benchmark output",
        count: 256
    ).joined(separator: "\n")
    let expectedContentDigest = digestHex(Data(expectedPlainText.utf8))
    var observedContentDigests = Set<String>()

    let streamID = NodeId(14)
    var decodeVisibleSamples = [Double]()
    var drawOnlySamples = [Double]()
    var offsetsExact = true
    var freshState = true
    var drawCompletions = 0

    for _ in 0..<iterations {
        let session = TerminalSession()
        let view = TerminalView(nodeID: streamID)
        let window = NSWindow(
            contentRect: view.frame,
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentView = view
        let blankBitmap = renderedBitmapSignature(view)

        let decodeStarted = clock.now
        let snapshot = try await session.applyData(
            streamID: streamID,
            byteOffset: 0,
            data: payload
        )
        let drawStarted = clock.now
        view.apply(snapshot)
        let visible: Bool
        if fullPaint {
            visible = try await benchmarkObserveOnScreenPaint(window)
                .crossedDisplayRefresh
        } else {
            visible = rasterize(view)
        }
        let visibleAt = clock.now
        let renderedBitmap = renderedBitmapSignature(view)
        let contentDrawn = blankBitmap != nil
            && renderedBitmap != nil
            && blankBitmap != renderedBitmap
        drawOnlySamples.append(milliseconds(drawStarted.duration(to: visibleAt)))
        decodeVisibleSamples.append(milliseconds(decodeStarted.duration(to: visibleAt)))
        if visible && contentDrawn {
            drawCompletions += 1
        }
        let fullPlainText = terminalFullPlainText(snapshot)
        let contentDigest = digestHex(Data(fullPlainText.utf8))
        observedContentDigests.insert(contentDigest)
        offsetsExact = offsetsExact && snapshot.nextOffset == UInt64(payload.count)
        freshState = freshState
            && snapshot.nextOffset == UInt64(payload.count)
            && fullPlainText == expectedPlainText
            && contentDigest == expectedContentDigest
        await session.acknowledgeRedraw(streamID: streamID)
        window.close()
    }

    return Section(
        id: "31.6",
        name: "Terminal",
        metrics: [
            metric("client Terminal decode-to-visible", p50(decodeVisibleSamples), id: "client_terminal.decode_visible"),
            metric("client Terminal decode-to-visible", percentile(decodeVisibleSamples, 0.95), "ms", "p95", id: "client_terminal.decode_visible"),
            metric("client Terminal decode-to-visible", percentile(decodeVisibleSamples, 0.99), "ms", "p99", id: "client_terminal.decode_visible"),
            metric("client Terminal draw-only", p50(drawOnlySamples), id: "client_terminal.draw_only"),
            metric("client Terminal draw-only", percentile(drawOnlySamples, 0.95), "ms", "p95", id: "client_terminal.draw_only"),
            metric("client Terminal draw-only", percentile(drawOnlySamples, 0.99), "ms", "p99", id: "client_terminal.draw_only"),
            metric("client terminal frame", Double(payload.count), "bytes", "exact", id: "client_terminal.frame_bytes"),
            metric("client terminal draw completions", Double(drawCompletions), "frames", "exact", id: "client_terminal.raster_completions"),
        ],
        assertions: [
            Assertion(
                id: "terminal_offsets_exact",
                name: "fresh embedded terminal offsets remain exact",
                passed: offsetsExact,
                detail: "every fresh TerminalSession accepted byte offset zero and ended at \(payload.count)"
            ),
            Assertion(
                id: "terminal_draw_completion",
                name: "embedded terminal visible completion is an actual draw",
                passed: drawCompletions == iterations,
                detail: "\(drawCompletions)/\(iterations) TerminalView completions crossed the configured visible boundary and produced a content-distinct bitmap"
            ),
            Assertion(
                id: "terminal_fresh_state",
                name: "terminal samples begin from fresh parser and view state",
                passed: freshState,
                detail: "each sample constructs a new TerminalSession, TerminalView, and NSWindow, then reproduces the exact 256-line plain text with SHA-256 \(expectedContentDigest); observed digests: \(observedContentDigests.sorted())"
            ),
        ],
        notes: [
            "Client parse/apply/decode-to-visible and draw-only components are reported separately. The Rust section provides the like-for-like standalone-versus-embedded end-to-end PTY comparison with the byte-identical ANSI payload; the consolidated report keeps these client-only components distinct and avoids cumulative-state bias.",
            "Smoke uses an explicitly named offscreen bitmap draw; full mode uses the visible-window framebuffer-advance boundary."
        ]
    )
}

@main
private struct BenchmarkDriver {
    @MainActor
    static func main() {
        let arguments: Arguments
        var parentWatchdog: BenchmarkParentLivenessWatchdog?
        do {
            arguments = try Arguments()
            if arguments.candidate != nil {
                guard setpgid(0, 0) == 0, getpgrp() == getpid() else {
                    throw BenchmarkFailure.message(
                        "renderer candidate could not establish process-group ownership: errno \(errno)"
                    )
                }
                guard let parentBirth = arguments.driverBirthUnixNanoseconds else {
                    throw BenchmarkFailure.message(
                        "renderer candidate has no parent birth identity"
                    )
                }
                let watchdog = try BenchmarkParentLivenessWatchdog(
                    parentPID: arguments.driverPID,
                    parentBirthUnixNanoseconds: parentBirth
                )
                try watchdog.start()
                parentWatchdog = watchdog
            }
        } catch {
            FileHandle.standardError.write(
                Data("BenchmarkDriver failed: \(error)\n".utf8)
            )
            Darwin.exit(EXIT_FAILURE)
        }
        defer { parentWatchdog?.cancel() }

        // Candidate process-group ownership and parent-birth supervision are established above,
        // synchronously, before this first AppKit access.
        let application = NSApplication.shared
        let applicationDelegate = BenchmarkApplicationDelegate()
        application.delegate = applicationDelegate
        application.setActivationPolicy(.accessory)
        application.finishLaunching()

        var failure: (any Error)?
        Task { @MainActor in
            do {
                try await run(arguments: arguments)
            } catch {
                failure = error
            }
            application.stop(nil)
            if let wakeEvent = NSEvent.otherEvent(
                with: .applicationDefined,
                location: .zero,
                modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: 0,
                context: nil,
                subtype: 0,
                data1: 0,
                data2: 0
            ) {
                application.postEvent(wakeEvent, atStart: false)
            }
        }
        application.run()
        application.delegate = nil
        withExtendedLifetime(applicationDelegate) {}
        if let failure {
            FileHandle.standardError.write(
                Data("BenchmarkDriver failed: \(failure)\n".utf8)
            )
            Darwin.exit(EXIT_FAILURE)
        }
    }

    @MainActor
    private static func run(arguments: Arguments) async throws {
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
                result = try await runSRUICandidate(
                    fixture: fixture,
                    operations: fixtureOperations,
                    iterations: iterations,
                    fullPaint: fullPaint,
                    driverPID: arguments.driverPID
                )
            case "webkit":
                result = try await runWebCandidate(
                    fixture: fixture,
                    iterations: iterations,
                    fullPaint: fullPaint,
                    driverPID: arguments.driverPID
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
        var rendererProcessAttribution = [RendererProcessAttribution]()
        if arguments.onlySection == nil || arguments.onlySection == "31.1" {
            let result = try localRenderer(
                fixture: fixture,
                fixtureURL: arguments.fixture,
                profile: arguments.profile
            )
            sections.append(result.section)
            rendererProcessAttribution = result.attributions
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
                    canonicalTransactionBytes: canonicalTransaction.count,
                    rendererProcessAttribution: rendererProcessAttribution
                ),
                sections: sections
            )
        ).write(to: arguments.output)
    }
}
