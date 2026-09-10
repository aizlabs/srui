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

@MainActor
final class NavigationProbe: NSObject, WKNavigationDelegate {
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

func escapedHTML(_ value: String) -> String {
    value
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
}

struct ParityNode: Codable, Equatable {
    let id: UInt64
    let type: String
    let parent: UInt64?
    let properties: [String: JSONScalar]
}

struct DOMInspection: Decodable {
    let nodes: [ParityNode]
    let elementKindsPassed: Bool
    let renderedPropertiesPassed: Bool
}

func fixtureParityNodes(_ fixture: Fixture) -> [ParityNode] {
    fixtureParityNodes(fixture.nodes)
}

func fixtureParityNodes(_ nodes: [FixtureNode]) -> [ParityNode] {
    nodes.map {
        ParityNode(
            id: $0.id,
            type: $0.type,
            parent: $0.parent,
            properties: $0.properties ?? [:]
        )
    }.sorted { $0.id < $1.id }
}

func encodedProperties(_ properties: [String: JSONScalar]) throws -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    return try encoder.encode(properties).base64EncodedString()
}

func html(for fixture: Fixture) throws -> String {
    try html(for: fixture.nodes)
}

func html(for nodes: [FixtureNode]) throws -> String {
    let byParent = Dictionary(grouping: nodes) { $0.parent }
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

func nativeSemanticParity(store: SemanticStore, fixture: Fixture) throws -> Bool {
    try nativeSemanticParity(store: store, nodes: fixture.nodes)
}

func nativeSemanticParity(
    store: SemanticStore,
    nodes: [FixtureNode]
) throws -> Bool {
    guard store.nodeCount == nodes.count else { return false }
    for expected in nodes {
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
func nativeRenderedPropertiesMatch(
    renderer: AppKitRenderer,
    fixture: Fixture
) -> Bool {
    nativeRenderedPropertiesMatch(renderer: renderer, nodes: fixture.nodes)
}

@MainActor
func nativeRenderedPropertiesMatch(
    renderer: AppKitRenderer,
    nodes: [FixtureNode]
) -> Bool {
    let expectedChildren = Dictionary(grouping: nodes.compactMap {
        node -> (UInt64, UInt64)? in
        node.parent.map { ($0, node.id) }
    }, by: { $0.0 }).mapValues { $0.map(\.1) }
    for expected in nodes {
        guard let handle = renderer.registry.handle(for: NodeId(expected.id)),
              handle.parentID?.value == expected.parent,
              handle.childIDs.map(\.value) == (expectedChildren[expected.id] ?? []) else {
            return false
        }
        if let parentID = expected.parent {
            guard let parent = renderer.registry.handle(for: NodeId(parentID)),
                  handle.view.isDescendant(of: parent.view) else {
                return false
            }
        } else if expected.type == "Surface", handle.window == nil {
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
final class TimedContinuation<Value: Sendable> {
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
final class AnimationFrameProbe: NSObject, WKScriptMessageHandler {
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
func nextAnimationFrame(in webView: WKWebView) async throws {
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
func snapshotRenderedPixels(_ webView: WKWebView) async throws -> Bool {
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
func javascriptString(_ script: String, in webView: WKWebView) async throws -> String {
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
func waitForWebContent(
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
func inspectDOM(in webView: WKWebView) async throws -> DOMInspection {
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
struct RendererCandidateResult: Codable {
    let candidate: String
    let firstPaint: [Double]
    let completePaint: [Double]
    let cpuTime: [Double]
    let hostLiveAllocationDelta: [Double]
    let allocatedFootprintGrowthMiB: Double
    let processFootprintPeak: [Double]
    let renderedNodeCount: Int
    let presentationCompletions: Int
    let representationBytes: Int
    let semanticParityPassed: Bool
    let elementKindsPassed: Bool
    let resourceAttributionComplete: Bool
    let captureAuthorization: Bool
    let pixelCaptureCompletions: Int
    let contentPresentationPassed: Bool
    let contentPresentationDetail: String
    let paintCompletionMode: String
    let attribution: RendererProcessAttribution
    let succeeded: Bool
}

struct ProgressiveTransactionPlan {
    let firstNodeCount: Int
    let firstNodes: [FixtureNode]
    let firstTransaction: SemanticModel.Transaction
    let completionTransaction: SemanticModel.Transaction
    let firstWireBytes: Data
    let completionWireBytes: Data
    let canonicalBytes: Data
}

func validatedFirstPaintNodeCount(_ fixture: Fixture) throws -> Int {
    let count = fixture.firstPaintNodeCount
    guard count > 0, count < fixture.nodes.count else {
        throw BenchmarkFailure.message(
            "first_paint_node_count must split the representative fixture into two non-empty commits"
        )
    }
    var seen = Set<UInt64>()
    for (index, node) in fixture.nodes.enumerated() {
        guard seen.insert(node.id).inserted else {
            throw BenchmarkFailure.message("representative fixture repeats node id \(node.id)")
        }
        if let parent = node.parent, seen.contains(parent) == false {
            throw BenchmarkFailure.message(
                "fixture node \(node.id) precedes its parent \(parent)"
            )
        }
        if index < count, let parent = node.parent,
           fixture.nodes.prefix(count).contains(where: { $0.id == parent }) == false {
            throw BenchmarkFailure.message(
                "first-paint fixture prefix is not a closed semantic subtree"
            )
        }
    }
    let usefulTypes = Set(["Text", "RichText", "Progress", "Tree", "TextArea", "Button"])
    guard fixture.nodes.prefix(count).contains(where: { usefulTypes.contains($0.type) }) else {
        throw BenchmarkFailure.message(
            "first-paint fixture prefix contains no useful visible control"
        )
    }
    return count
}

func lengthPrefixedTransactionSequence(_ payloads: [Data]) -> Data {
    var result = Data()
    for payload in payloads {
        var length = UInt64(payload.count).bigEndian
        withUnsafeBytes(of: &length) {
            result.append(contentsOf: $0)
        }
        result.append(payload)
    }
    return result
}

func progressiveTransactionPlan(
    fixture: Fixture,
    operations: [SemanticModel.Operation]
) throws -> ProgressiveTransactionPlan {
    guard operations.count == fixture.nodes.count else {
        throw BenchmarkFailure.message(
            "fixture operation count does not match fixture node count"
        )
    }
    let firstNodeCount = try validatedFirstPaintNodeCount(fixture)
    let firstOperations = Array(operations.prefix(firstNodeCount))
    let completionOperations = Array(operations.dropFirst(firstNodeCount))
    let firstTransaction = SemanticModel.Transaction(
        baseRevision: Revision(0),
        operations: firstOperations
    )
    let completionTransaction = SemanticModel.Transaction(
        baseRevision: Revision(1),
        operations: completionOperations
    )
    let firstWireBytes = try firstTransaction.toWire().serializedData()
    let completionWireBytes = try completionTransaction.toWire().serializedData()
    return ProgressiveTransactionPlan(
        firstNodeCount: firstNodeCount,
        firstNodes: Array(fixture.nodes.prefix(firstNodeCount)),
        firstTransaction: firstTransaction,
        completionTransaction: completionTransaction,
        firstWireBytes: firstWireBytes,
        completionWireBytes: completionWireBytes,
        canonicalBytes: lengthPrefixedTransactionSequence([
            firstWireBytes,
            completionWireBytes,
        ])
    )
}

struct ProgressiveContentEvidenceCheck {
    let passed: Bool
    let detail: String
}

func progressiveContentEvidenceCheck(
    first: OnScreenPaintObservation,
    complete: OnScreenPaintObservation,
    fullPaint: Bool
) -> ProgressiveContentEvidenceCheck {
    guard fullPaint else {
        return ProgressiveContentEvidenceCheck(
            passed: first.crossedDisplayRefresh && complete.crossedDisplayRefresh,
            detail: "smoke-only same-instance semantic/control validation plus separate offscreen raster or WKSnapshot completions; no WindowServer, compositor, visibility, or captured-content claim"
        )
    }
    guard first.crossedDisplayRefresh,
          complete.crossedDisplayRefresh,
          first.captureAuthorization,
          complete.captureAuthorization,
          first.pixelCaptureVerified,
          complete.pixelCaptureVerified,
          let firstEvidence = first.compositedContentEvidence,
          let completeEvidence = complete.compositedContentEvidence else {
        return ProgressiveContentEvidenceCheck(
            passed: false,
            detail: "full-paint observation lacked authorized composited client-content evidence; first provenance=\(first.visibilityProvenance), complete provenance=\(complete.visibilityProvenance)"
        )
    }
    let sameGeometry =
        firstEvidence.pixelWidth == completeEvidence.pixelWidth
            && firstEvidence.pixelHeight == completeEvidence.pixelHeight
            && firstEvidence.unmaskedPixelCount == completeEvidence.unmaskedPixelCount
    let usefulContent =
        firstEvidence.hasNonblankContent
            && firstEvidence.hasNonuniformContent
            && completeEvidence.hasNonblankContent
            && completeEvidence.hasNonuniformContent
    let distinctContent =
        firstEvidence.normalizedFingerprintSHA256
            != completeEvidence.normalizedFingerprintSHA256
    let passed = sameGeometry && usefulContent && distinctContent
    return ProgressiveContentEvidenceCheck(
        passed: passed,
        detail: "full composited client-content proof: first=\(String(firstEvidence.normalizedFingerprintSHA256.prefix(16))) complete=\(String(completeEvidence.normalizedFingerprintSHA256.prefix(16))) dimensions=\(firstEvidence.pixelWidth)x\(firstEvidence.pixelHeight)/\(completeEvidence.pixelWidth)x\(completeEvidence.pixelHeight) unmasked=\(firstEvidence.unmaskedPixelCount)/\(completeEvidence.unmaskedPixelCount) quantized-colors=\(firstEvidence.distinctQuantizedColorCount)/\(completeEvidence.distinctQuantizedColorCount) non-dominant=\(firstEvidence.nonDominantPixelCount)/\(completeEvidence.nonDominantPixelCount) nonblank-and-nonuniform=\(usefulContent) distinct=\(distinctContent) same-geometry=\(sameGeometry) provenance=\(first.visibilityProvenance)/\(complete.visibilityProvenance)"
    )
}

@MainActor
func configureNativeBenchmarkGeometry(_ renderer: AppKitRenderer) throws {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1 else {
        throw BenchmarkFailure.message(
            "representative native fixture must mount exactly one surface window"
        )
    }
    for window in windows {
        window.setContentSize(NSSize(width: 960, height: 720))
        window.contentView?.layoutSubtreeIfNeeded()
    }
}

@MainActor
private func explicitNativePaintTarget(
    _ renderer: AppKitRenderer
) throws -> BenchmarkExplicitPaintTarget {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1,
          let window = windows.first,
          let contentView = window.contentView,
          window.isVisible == false else {
        throw BenchmarkFailure.message(
            "native explicit paint must produce one hidden drawable surface"
        )
    }
    return BenchmarkExplicitPaintTarget(
        window: window,
        targetView: contentView
    )
}

@MainActor
private func benchmarkMainScreen() throws -> NSScreen {
    guard let screen = NSScreen.main else {
        throw BenchmarkFailure.message(
            "full renderer benchmark requires a main display"
        )
    }
    return screen
}

@MainActor
func observeNativePresentation(
    _ renderer: AppKitRenderer,
    fullPaint: Bool,
    onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)? = nil
) async throws -> OnScreenPaintObservation {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1, let window = windows.first else {
        throw BenchmarkFailure.message(
            "representative native fixture must mount exactly one presentation window"
        )
    }
    guard let contentView = window.contentView else {
        throw BenchmarkFailure.message("native renderer surface has no drawable content view")
    }
    let probe = BenchmarkDrawCompletionProbe(frame: contentView.bounds)
    probe.autoresizingMask = [.width, .height]
    contentView.addSubview(probe, positioned: .above, relativeTo: nil)
    defer {
        probe.removeFromSuperview()
    }

    if fullPaint {
        return try await benchmarkObserveOnScreenPaint(
            window,
            contentDrawProbe: probe,
            onPresented: onPresented
        )
    }

    let drawCount = probe.drawCount
    let rasterized = rasterizeRenderer(renderer, showWindows: false)
    let presentedAt = clock.now
    let presentedUnixNanoseconds = benchmarkWallClockNanoseconds()
    onPresented?(presentedAt, presentedUnixNanoseconds)
    return OnScreenPaintObservation(
        crossedDisplayRefresh: rasterized && probe.drawCount > drawCount,
        captureAuthorization: false,
        pixelCaptureVerified: false,
        presentedAt: presentedAt,
        visibilityProvenance: "offscreen_appkit_raster_smoke_only",
        compositedContentEvidence: nil
    )
}

@MainActor
private func applyNativeFirstState(
    plan: ProgressiveTransactionPlan,
    renderer: AppKitRenderer
) throws -> SemanticStore {
    let firstWire = try SRUITransaction(
        serializedBytes: plan.firstWireBytes
    )
    let firstTransaction = try ProtocolDecoder()
        .validateAndConvertTransaction(wire: firstWire)
    var store = SemanticStore()
    guard case .success = store.applyTransactionRecord(firstTransaction) else {
        throw BenchmarkFailure.message(
            "first native transaction failed semantic application"
        )
    }
    try renderer.attach(store: store)
    try configureNativeBenchmarkGeometry(renderer)
    return store
}

@MainActor
private func applyNativeCompleteState(
    plan: ProgressiveTransactionPlan,
    renderer: AppKitRenderer
) throws -> SemanticStore {
    var store = try applyNativeFirstState(
        plan: plan,
        renderer: renderer
    )
    let completionWire = try SRUITransaction(
        serializedBytes: plan.completionWireBytes
    )
    let completionTransaction = try ProtocolDecoder()
        .validateAndConvertTransaction(wire: completionWire)
    guard case .success = store.applyTransactionRecord(
        completionTransaction
    ) else {
        throw BenchmarkFailure.message(
            "complete native transaction failed semantic application"
        )
    }
    try renderer.apply(
        transaction: completionTransaction,
        newStore: store
    )
    try configureNativeBenchmarkGeometry(renderer)
    return store
}

@MainActor
private func submitNativeRendererForDisplay(
    _ renderer: AppKitRenderer
) throws {
    let windows = renderer.registry.surfaceHandles.compactMap(\.window)
    guard windows.count == 1 else {
        throw BenchmarkFailure.message(
            "native display-submission pass requires exactly one surface"
        )
    }
    renderer.showWindows()
    for window in windows {
        window.orderFrontRegardless()
        window.contentView?.layoutSubtreeIfNeeded()
        window.contentView?.displayIfNeeded()
        window.displayIfNeeded()
    }
    CATransaction.flush()
}

@MainActor
func runSRUICandidate(
    fixture: Fixture,
    operations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool,
    driverPID: Int32,
    allocationControl: AllocationCaptureControl? = nil
) async throws -> RendererCandidateResult {
    let plan = try progressiveTransactionPlan(
        fixture: fixture,
        operations: operations
    )
    guard let warmOperation = operations.first else {
        throw BenchmarkFailure.message(
            "representative native fixture has no warm operation"
        )
    }
    let warmStore = try makeStore([warmOperation])
    let processIDs = [getpid()]
    guard let initialHostIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message(
            "native candidate process birth identity was unavailable"
        )
    }

    func makeWarmedRenderer() async throws -> AppKitRenderer {
        let renderer = AppKitRenderer()
        do {
            try renderer.attach(store: warmStore)
            try configureNativeBenchmarkGeometry(renderer)
            let warmObservation = try await observeNativePresentation(
                renderer,
                fullPaint: false
            )
            guard warmObservation.crossedDisplayRefresh,
                  renderer.registry.surfaceHandles.allSatisfy({
                      $0.window?.isVisible == false
                  }) else {
                throw BenchmarkFailure.message(
                    "native sample renderer did not warm offscreen"
                )
            }
            try renderer.attach(store: SemanticStore())
            guard renderer.registry.surfaceHandles.isEmpty else {
                throw BenchmarkFailure.message(
                    "native sample renderer did not reset to an empty hidden state"
                )
            }
            return renderer
        } catch {
            closeRenderer(renderer)
            throw error
        }
    }

    let startedUnixNanoseconds = benchmarkWallClockNanoseconds()
    var first = [Double]()
    var complete = [Double]()
    var cpu = [Double]()
    var allocations = [Double]()
    var growth = [Double]()
    var peaks = [Double]()
    var measurementIntervals = [RendererMeasurementInterval]()
    var renderedNodeCount = 0
    var presentationCompletions = 0
    var pixelCaptureCompletions = 0
    var captureAuthorization = fullPaint
    var semanticParityPassed = true
    var renderedPropertiesPassed = true
    var resourceAttributionComplete = true
    var contentPresentationPassed = true
    var contentPresentationPassCount = 0
    var lastContentPresentationDetail = ""
    var firstContentPresentationFailure: String?

    for index in 0..<iterations {
        let firstObservation: OnScreenPaintObservation
        do {
            let renderer = try await makeWarmedRenderer()
            defer {
                closeRenderer(renderer)
            }
            let store: SemanticStore
            if fullPaint {
                var measuredStore: SemanticStore?
                let measurement =
                    try await benchmarkMeasureExplicitCompositedPaint(
                        on: try benchmarkMainScreen()
                    ) {
                        let candidateStore = try applyNativeFirstState(
                            plan: plan,
                            renderer: renderer
                        )
                        measuredStore = candidateStore
                        return try explicitNativePaintTarget(renderer)
                    }
                guard let candidateStore = measuredStore else {
                    throw BenchmarkFailure.message(
                        "first-paint native action produced no semantic store"
                    )
                }
                store = candidateStore
                firstObservation = measurement.observation
                first.append(
                    measurement.presentationLatencyMilliseconds
                )
            } else {
                let firstStarted = clock.now
                store = try applyNativeFirstState(
                    plan: plan,
                    renderer: renderer
                )
                firstObservation = try await observeNativePresentation(
                    renderer,
                    fullPaint: false
                )
                first.append(
                    milliseconds(
                        firstStarted.duration(
                            to: firstObservation.presentedAt
                        )
                    )
                )
            }
            let firstSemanticParity = try nativeSemanticParity(
                store: store,
                nodes: plan.firstNodes
            )
            semanticParityPassed =
                semanticParityPassed && firstSemanticParity
            renderedPropertiesPassed =
                renderedPropertiesPassed
                    && nativeRenderedPropertiesMatch(
                        renderer: renderer,
                        nodes: plan.firstNodes
                    )
            if firstObservation.crossedDisplayRefresh {
                presentationCompletions += 1
            }
            if firstObservation.pixelCaptureVerified {
                pixelCaptureCompletions += 1
            }
            captureAuthorization = captureAuthorization
                && firstObservation.captureAuthorization
        }

        let completeObservation: OnScreenPaintObservation
        do {
            let renderer = try await makeWarmedRenderer()
            defer {
                closeRenderer(renderer)
            }
            let store: SemanticStore
            if fullPaint {
                var measuredStore: SemanticStore?
                let measurement =
                    try await benchmarkMeasureExplicitCompositedPaint(
                        on: try benchmarkMainScreen()
                    ) {
                        let candidateStore = try applyNativeCompleteState(
                            plan: plan,
                            renderer: renderer
                        )
                        measuredStore = candidateStore
                        return try explicitNativePaintTarget(renderer)
                    }
                guard let candidateStore = measuredStore else {
                    throw BenchmarkFailure.message(
                        "complete-paint native action produced no semantic store"
                    )
                }
                store = candidateStore
                completeObservation = measurement.observation
                complete.append(
                    measurement.presentationLatencyMilliseconds
                )
            } else {
                let completeStarted = clock.now
                store = try applyNativeCompleteState(
                    plan: plan,
                    renderer: renderer
                )
                completeObservation = try await observeNativePresentation(
                    renderer,
                    fullPaint: false
                )
                complete.append(
                    milliseconds(
                        completeStarted.duration(
                            to: completeObservation.presentedAt
                        )
                    )
                )
            }
            let completeSemanticParity = try nativeSemanticParity(
                store: store,
                fixture: fixture
            )
            semanticParityPassed =
                semanticParityPassed && completeSemanticParity
            renderedPropertiesPassed =
                renderedPropertiesPassed
                    && nativeRenderedPropertiesMatch(
                        renderer: renderer,
                        fixture: fixture
                    )
            renderedNodeCount = renderer.registry.allHandles.count
            if completeObservation.crossedDisplayRefresh {
                presentationCompletions += 1
            }
            if completeObservation.pixelCaptureVerified {
                pixelCaptureCompletions += 1
            }
            captureAuthorization = captureAuthorization
                && completeObservation.captureAuthorization
        }

        let contentCheck = progressiveContentEvidenceCheck(
            first: firstObservation,
            complete: completeObservation,
            fullPaint: fullPaint
        )
        contentPresentationPassed =
            contentPresentationPassed && contentCheck.passed
        if contentCheck.passed {
            contentPresentationPassCount += 1
        } else if firstContentPresentationFailure == nil {
            firstContentPresentationFailure =
                "sample \(index): \(contentCheck.detail)"
        }
        lastContentPresentationDetail =
            "sample \(index): \(contentCheck.detail)"

        do {
            let renderer = try await makeWarmedRenderer()
            defer {
                closeRenderer(renderer)
            }
            let allocationToken = try await allocationControl?.begin(
                candidate: "srui",
                sampleIndex: index,
                targetPIDsByRole: ["host": getpid()]
            )
            var allocationCaptureFinished = false
            let beforeAllocator = mallocSample()
            let beforeResources =
                benchmarkProcessResourceSample(pids: processIDs)
            let sampleStartedUnixNanoseconds =
                benchmarkWallClockNanoseconds()

            do {
                let store = try applyNativeCompleteState(
                    plan: plan,
                    renderer: renderer
                )
                try submitNativeRendererForDisplay(renderer)
                let sampleEndedUnixNanoseconds =
                    benchmarkWallClockNanoseconds()
                let afterAllocator = mallocSample()
                let afterResources =
                    benchmarkProcessResourceSample(pids: processIDs)

                if let allocationToken, let allocationControl {
                    try await allocationControl.finish(
                        allocationToken,
                        startedUnixNanoseconds:
                            sampleStartedUnixNanoseconds,
                        endedUnixNanoseconds:
                            sampleEndedUnixNanoseconds
                    )
                    allocationCaptureFinished = true
                }
                measurementIntervals.append(
                    RendererMeasurementInterval(
                        startedUnixNanoseconds:
                            sampleStartedUnixNanoseconds,
                        endedUnixNanoseconds:
                            sampleEndedUnixNanoseconds,
                        requiredAllocationPIDs:
                            try requiredAllocationPIDs(
                                control: allocationControl,
                                token: allocationToken
                            )
                    )
                )
                cpu.append(
                    max(
                        0,
                        afterResources.cpuMilliseconds
                            - beforeResources.cpuMilliseconds
                    )
                )
                allocations.append(
                    Double(
                        max(
                            0,
                            afterAllocator.blocks
                                - beforeAllocator.blocks
                        )
                    )
                )
                growth.append(
                    max(
                        0,
                        afterResources.physicalFootprintMiB
                            - beforeResources.physicalFootprintMiB
                    )
                )
                resourceAttributionComplete =
                    resourceAttributionComplete
                        && beforeResources.measuredPIDCount
                            == processIDs.count
                        && afterResources.measuredPIDCount
                            == processIDs.count
                let resourceSemanticParity = try nativeSemanticParity(
                    store: store,
                    fixture: fixture
                )
                semanticParityPassed =
                    semanticParityPassed && resourceSemanticParity
                renderedPropertiesPassed =
                    renderedPropertiesPassed
                        && nativeRenderedPropertiesMatch(
                            renderer: renderer,
                            fixture: fixture
                        )
            } catch {
                if allocationCaptureFinished == false,
                   let allocationToken,
                   let allocationControl {
                    try? await allocationControl.finish(
                        allocationToken,
                        startedUnixNanoseconds:
                            sampleStartedUnixNanoseconds,
                        endedUnixNanoseconds:
                            benchmarkWallClockNanoseconds()
                    )
                }
                throw error
            }
        }

        do {
            let renderer = try await makeWarmedRenderer()
            defer {
                closeRenderer(renderer)
            }
            let footprintSampler = ProcessFootprintSampler(
                processIDs: processIDs
            )
            var footprintSamplerFinished = false
            defer {
                if footprintSamplerFinished == false {
                    footprintSampler.cancel()
                }
            }
            let peakStartedUnixNanoseconds =
                benchmarkWallClockNanoseconds()
            footprintSampler.begin(at: peakStartedUnixNanoseconds)

            let store = try applyNativeCompleteState(
                plan: plan,
                renderer: renderer
            )
            try submitNativeRendererForDisplay(renderer)
            // Always include a synchronous post-workload exact-PID sample;
            // a sub-millisecond pass must not pass with baseline-only peak data.
            footprintSampler.sampleNow()
            let peakEndedUnixNanoseconds =
                benchmarkWallClockNanoseconds()
            let footprintMeasurement = footprintSampler.finish(
                at: peakEndedUnixNanoseconds
            )
            footprintSamplerFinished = true

            peaks.append(
                footprintMeasurement.peakPhysicalFootprintMiB
            )
            resourceAttributionComplete =
                resourceAttributionComplete
                    && footprintMeasurement.sampleCount >= 2
                    && footprintMeasurement.allTargetProcessesMeasured
            let peakSemanticParity = try nativeSemanticParity(
                store: store,
                fixture: fixture
            )
            semanticParityPassed =
                semanticParityPassed && peakSemanticParity
            renderedPropertiesPassed =
                renderedPropertiesPassed
                    && nativeRenderedPropertiesMatch(
                        renderer: renderer,
                        fixture: fixture
                    )
        }
    }

    let endedUnixNanoseconds = benchmarkWallClockNanoseconds()
    guard let finalHostIdentity = benchmarkProcessIdentity(pid: getpid()),
          finalHostIdentity.birthUnixNanoseconds
            == initialHostIdentity.birthUnixNanoseconds,
          finalHostIdentity.observedAliveThroughUnixNanoseconds
            >= endedUnixNanoseconds else {
        throw BenchmarkFailure.message(
            "native candidate host identity was not alive through all passes"
        )
    }
    let attribution = RendererProcessAttribution(
        candidate: "srui",
        driverPID: driverPID,
        hostPID: getpid(),
        helperPIDs: [],
        processIdentities: [finalHostIdentity],
        startedUnixNanoseconds: startedUnixNanoseconds,
        endedUnixNanoseconds: endedUnixNanoseconds,
        measurementIntervals: measurementIntervals,
        helperPIDSource:
            "native candidate has no renderer helper processes"
    )
    let contentPresentationDetail =
        "\(contentPresentationPassCount)/\(iterations) samples passed; "
            + (
                firstContentPresentationFailure
                    ?? lastContentPresentationDetail
            )
    let requiredPixelCaptures = fullPaint ? iterations * 2 : 0
    return RendererCandidateResult(
        candidate: "srui",
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        hostLiveAllocationDelta: allocations,
        allocatedFootprintGrowthMiB: p50(growth),
        processFootprintPeak: peaks,
        renderedNodeCount: renderedNodeCount,
        presentationCompletions: presentationCompletions,
        representationBytes: plan.canonicalBytes.count,
        semanticParityPassed: semanticParityPassed,
        elementKindsPassed: renderedPropertiesPassed,
        resourceAttributionComplete: resourceAttributionComplete,
        captureAuthorization: captureAuthorization,
        pixelCaptureCompletions: pixelCaptureCompletions,
        contentPresentationPassed: contentPresentationPassed,
        contentPresentationDetail: contentPresentationDetail,
        paintCompletionMode: fullPaint
            ? "four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before production protobuf decode/apply/render and ends at the accepted ScreenCaptureKit frame displayTime; CPU/live-allocation/footprint-growth use a separate production decode/apply/display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only production decode/apply/display-submission pass"
            : "smoke-only first-state and complete-sequence timings end at separate offscreen AppKit raster completions; CPU/live-allocation/footprint-growth and peak use separate production decode/apply/display-submission passes, while no on-screen/compositor latency claim is made",
        attribution: attribution,
        succeeded: first.count == iterations
            && complete.count == iterations
            && cpu.count == iterations
            && allocations.count == iterations
            && peaks.count == iterations
            && renderedNodeCount == fixture.nodes.count
            && presentationCompletions == iterations * 2
            && semanticParityPassed
            && renderedPropertiesPassed
            && resourceAttributionComplete
            && contentPresentationPassed
            && (!fullPaint || (
                captureAuthorization
                    && pixelCaptureCompletions == requiredPixelCaptures
            ))
    )
}
@MainActor
func loadWebDOMState(
    _ webView: WKWebView,
    probe: NavigationProbe,
    html: Data,
    expectedParity: [ParityNode],
    phase: String,
    requireAnimationFrame: Bool
) async throws {
    probe.reset()
    webView.load(
        html,
        mimeType: "text/html",
        characterEncodingName: "utf-8",
        baseURL: URL(fileURLWithPath: "/", isDirectory: true)
    )
    // JavaScript evaluation yields the main actor while WebKit begins the
    // navigation; a synchronous run-loop wait before this point can starve
    // the navigation delegate in a command-line AppKit host.
    try await waitForWebContent(
        expectedNodeID: expectedParity.last?.id,
        requireAnimationFrame: false,
        in: webView
    )
    guard pumpWebView(
        webView,
        until: { probe.finished != nil || probe.failure != nil },
        timeout: 10
    ), probe.failure == nil else {
        throw probe.failure ?? BenchmarkFailure.message(
            "WKWebView \(phase) navigation timed out"
        )
    }
    if requireAnimationFrame {
        try await nextAnimationFrame(in: webView)
    }
}

private struct WebDOMLoadRequest {
    let html: Data
    let expectedParity: [ParityNode]
    let phase: String
}

private struct WebPaintMeasurement {
    let presentationLatencyMilliseconds: Double
    let observation: OnScreenPaintObservation
}

@MainActor
private func hideBenchmarkWindow(_ window: NSWindow) throws {
    window.orderOut(nil)
    CATransaction.flush()
    guard window.isVisible == false else {
        throw BenchmarkFailure.message(
            "benchmark window did not return to its hidden reset state"
        )
    }
}

@MainActor
private func submitWebViewForDisplay(
    _ webView: WKWebView,
    window: NSWindow
) throws {
    guard window.isVisible == false,
          webView.window === window else {
        throw BenchmarkFailure.message(
            "WebKit display-submission pass requires a hidden attached view"
        )
    }
    NSApplication.shared.activate()
    window.animationBehavior = .none
    window.makeKeyAndOrderFront(nil)
    window.orderFrontRegardless()
    webView.layoutSubtreeIfNeeded()
    webView.displayIfNeeded()
    window.displayIfNeeded()
    CATransaction.flush()
}

@MainActor
private func loadAndObserveWebStates(
    _ webView: WKWebView,
    window: NSWindow,
    probe: NavigationProbe,
    states: [WebDOMLoadRequest],
    fullPaint: Bool
) async throws -> WebPaintMeasurement {
    guard states.isEmpty == false else {
        throw BenchmarkFailure.message(
            "WebKit paint measurement requires at least one DOM state"
        )
    }
    if fullPaint {
        guard window.isVisible == false else {
            throw BenchmarkFailure.message(
                "WebKit explicit paint must begin with a hidden window"
            )
        }
        let measurement =
            try await benchmarkMeasureExplicitCompositedPaint(
                on: try benchmarkMainScreen()
            ) {
                for state in states {
                    try await loadWebDOMState(
                        webView,
                        probe: probe,
                        html: state.html,
                        expectedParity: state.expectedParity,
                        phase: state.phase,
                        requireAnimationFrame: false
                    )
                }
                webView.layoutSubtreeIfNeeded()
                return BenchmarkExplicitPaintTarget(
                    window: window,
                    targetView: webView
                )
            }
        return WebPaintMeasurement(
            presentationLatencyMilliseconds:
                measurement.presentationLatencyMilliseconds,
            observation: measurement.observation
        )
    }

    let started = clock.now
    for state in states {
        try await loadWebDOMState(
            webView,
            probe: probe,
            html: state.html,
            expectedParity: state.expectedParity,
            phase: state.phase,
            requireAnimationFrame: false
        )
    }
    let rendered = try await snapshotRenderedPixels(webView)
    let presentedAt = clock.now
    return WebPaintMeasurement(
        presentationLatencyMilliseconds:
            milliseconds(started.duration(to: presentedAt)),
        observation: OnScreenPaintObservation(
            crossedDisplayRefresh: rendered,
            captureAuthorization: false,
            pixelCaptureVerified: false,
            presentedAt: presentedAt,
            visibilityProvenance:
                "offscreen_wk_snapshot_smoke_only",
            compositedContentEvidence: nil
        )
    )
}

@MainActor
func runWebCandidate(
    fixture: Fixture,
    iterations: Int,
    fullPaint: Bool,
    driverPID: Int32,
    allocationControl: AllocationCaptureControl? = nil
) async throws -> RendererCandidateResult {
    let firstNodeCount = try validatedFirstPaintNodeCount(fixture)
    let firstNodes = Array(fixture.nodes.prefix(firstNodeCount))
    let firstRepresentation = Data(try html(for: firstNodes).utf8)
    let completeRepresentation = Data(try html(for: fixture).utf8)
    let firstParity = fixtureParityNodes(firstNodes)
    let completeParity = fixtureParityNodes(fixture)
    let warmRepresentation = Data(
        (
            "<!doctype html><html><body><main data-srui-id=\"0\" "
                + "data-srui-type=\"Surface\" data-srui-properties=\"e30=\" "
                + "role=\"application\"></main></body></html>"
        ).utf8
    )
    let warmParity = [
        ParityNode(
            id: 0,
            type: "Surface",
            parent: nil,
            properties: [:]
        )
    ]
    let resetRepresentation = Data(
        "<!doctype html><html><body></body></html>".utf8
    )
    guard let hostIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message(
            "WebKit candidate process birth identity was unavailable"
        )
    }

    let firstVisualStates = [
        WebDOMLoadRequest(
            html: firstRepresentation,
            expectedParity: firstParity,
            phase: "first useful subtree"
        )
    ]
    let completeVisualStates = [
        WebDOMLoadRequest(
            html: firstRepresentation,
            expectedParity: firstParity,
            phase: "complete-pass first DOM state"
        ),
        WebDOMLoadRequest(
            html: completeRepresentation,
            expectedParity: completeParity,
            phase: "complete representative state"
        ),
    ]

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
    var measurementIntervals = [RendererMeasurementInterval]()
    var renderedNodeCount = 0
    var presentationCompletions = 0
    var pixelCaptureCompletions = 0
    var captureAuthorization = fullPaint
    var semanticParityPassed = true
    var elementKindsPassed = true
    var resourceAttributionComplete = true
    var contentPresentationPassed = true
    var contentPresentationPassCount = 0
    var lastContentPresentationDetail = ""
    var firstContentPresentationFailure: String?

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
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: 960,
                height: 720
            ),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentView = webView
        defer {
            webView.stopLoading()
            webView.navigationDelegate = nil
            window.contentView = nil
            window.close()
            pumpRunLoop(for: 0.05)
        }

        guard window.isVisible == false else {
            throw BenchmarkFailure.message(
                "WebKit sample window was visible before warm-up"
            )
        }
        try await loadWebDOMState(
            webView,
            probe: probe,
            html: warmRepresentation,
            expectedParity: warmParity,
            phase: "offscreen warm",
            requireAnimationFrame: false
        )
        let warmRasterized = try await snapshotRenderedPixels(webView)
        let warmInspection = try await inspectDOM(in: webView)
        guard warmRasterized,
              window.isVisible == false,
              warmInspection.nodes == warmParity,
              warmInspection.elementKindsPassed,
              warmInspection.renderedPropertiesPassed else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not warm offscreen"
            )
        }
        try await loadWebDOMState(
            webView,
            probe: probe,
            html: resetRepresentation,
            expectedParity: [],
            phase: "pre-first hidden reset",
            requireAnimationFrame: false
        )
        guard window.isVisible == false,
              (try await inspectDOM(in: webView)).nodes.isEmpty else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not reset while hidden"
            )
        }

        let firstMeasurement = try await loadAndObserveWebStates(
            webView,
            window: window,
            probe: probe,
            states: firstVisualStates,
            fullPaint: fullPaint
        )
        let firstObservation = firstMeasurement.observation
        first.append(
            firstMeasurement.presentationLatencyMilliseconds
        )
        let firstInspection = try await inspectDOM(in: webView)
        semanticParityPassed = semanticParityPassed
            && firstInspection.nodes == firstParity
        elementKindsPassed = elementKindsPassed
            && firstInspection.elementKindsPassed
            && firstInspection.renderedPropertiesPassed
        if firstObservation.crossedDisplayRefresh {
            presentationCompletions += 1
        }
        if firstObservation.pixelCaptureVerified {
            pixelCaptureCompletions += 1
        }
        captureAuthorization = captureAuthorization
            && firstObservation.captureAuthorization
        if fullPaint {
            try hideBenchmarkWindow(window)
        }

        try await loadWebDOMState(
            webView,
            probe: probe,
            html: resetRepresentation,
            expectedParity: [],
            phase: "pre-complete hidden reset",
            requireAnimationFrame: false
        )
        guard window.isVisible == false,
              (try await inspectDOM(in: webView)).nodes.isEmpty else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not reset before complete visual pass"
            )
        }

        let completeMeasurement =
            try await loadAndObserveWebStates(
                webView,
                window: window,
                probe: probe,
                states: completeVisualStates,
                fullPaint: fullPaint
            )
        let completeObservation = completeMeasurement.observation
        complete.append(
            completeMeasurement.presentationLatencyMilliseconds
        )
        let completeInspection = try await inspectDOM(in: webView)
        renderedNodeCount = completeInspection.nodes.count
        semanticParityPassed = semanticParityPassed
            && completeInspection.nodes == completeParity
        elementKindsPassed = elementKindsPassed
            && completeInspection.elementKindsPassed
            && completeInspection.renderedPropertiesPassed
        if completeObservation.crossedDisplayRefresh {
            presentationCompletions += 1
        }
        if completeObservation.pixelCaptureVerified {
            pixelCaptureCompletions += 1
        }
        captureAuthorization = captureAuthorization
            && completeObservation.captureAuthorization
        if fullPaint {
            try hideBenchmarkWindow(window)
        }

        let contentCheck = progressiveContentEvidenceCheck(
            first: firstObservation,
            complete: completeObservation,
            fullPaint: fullPaint
        )
        contentPresentationPassed =
            contentPresentationPassed && contentCheck.passed
        if contentCheck.passed {
            contentPresentationPassCount += 1
        } else if firstContentPresentationFailure == nil {
            firstContentPresentationFailure =
                "sample \(index): \(contentCheck.detail)"
        }
        lastContentPresentationDetail =
            "sample \(index): \(contentCheck.detail)"

        try await loadWebDOMState(
            webView,
            probe: probe,
            html: resetRepresentation,
            expectedParity: [],
            phase: "pre-resource hidden reset",
            requireAnimationFrame: false
        )
        guard window.isVisible == false,
              (try await inspectDOM(in: webView)).nodes.isEmpty else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not reset before resource pass"
            )
        }

        let beforeHelperPIDsByRole =
            benchmarkWebKitHelperProcessIDsByRole(webView)
        let beforeHelperPIDs = Set(beforeHelperPIDsByRole.values)
            .subtracting([getpid()])
        let beforeHelperIdentities = beforeHelperPIDs.compactMap {
            benchmarkProcessIdentity(pid: $0)
        }
        guard let sampleWebContentPID =
                beforeHelperPIDsByRole["webcontent"],
              beforeHelperPIDs.contains(sampleWebContentPID) else {
            throw BenchmarkFailure.message(
                "WebKit sample has no exact pre-measurement WebContent PID"
            )
        }
        let measurementPIDs = [getpid()] + beforeHelperPIDs.sorted()
        var allocationTargetPIDsByRole = beforeHelperPIDsByRole
        allocationTargetPIDsByRole["host"] = getpid()
        if let allocationControl,
           let forcedAbsentRole = ProcessInfo.processInfo.environment[
               "SRUI_BENCHMARK_FORCE_ABSENT_OPTIONAL_ALLOCATION_ROLE"
           ] {
            guard forcedAbsentRole == allocationControl.targetRole,
                  forcedAbsentRole == "network"
                    || forcedAbsentRole == "gpu" else {
                throw BenchmarkFailure.message(
                    "forced absent allocation role must match a targeted "
                        + "optional role"
                )
            }
            allocationTargetPIDsByRole.removeValue(
                forKey: forcedAbsentRole
            )
        }
        var sampleResourcesComplete =
            beforeHelperPIDs.isEmpty == false
                && beforeHelperIdentities.count
                    == beforeHelperPIDs.count
        for identity in beforeHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds
                    != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
        }
        helperPIDs.formUnion(beforeHelperPIDs)

        let allocationToken = try await allocationControl?.begin(
            candidate: "webkit",
            sampleIndex: index,
            targetPIDsByRole: allocationTargetPIDsByRole
        )
        var allocationCaptureFinished = false
        let beforeAllocator = mallocSample()
        let beforeResources = benchmarkProcessResourceSample(
            pids: measurementPIDs
        )
        sampleResourcesComplete =
            sampleResourcesComplete
                && beforeResources.measuredPIDCount
                    == measurementPIDs.count
        let sampleStartedUnixNanoseconds =
            benchmarkWallClockNanoseconds()
        var resourceEndedUnixNanoseconds: UInt64?

        do {
            for state in completeVisualStates {
                try await loadWebDOMState(
                    webView,
                    probe: probe,
                    html: state.html,
                    expectedParity: state.expectedParity,
                    phase: "resource \(state.phase)",
                    requireAnimationFrame: false
                )
            }
            try submitWebViewForDisplay(webView, window: window)
            let ended = benchmarkWallClockNanoseconds()
            resourceEndedUnixNanoseconds = ended
            let afterAllocator = mallocSample()
            let afterResources = benchmarkProcessResourceSample(
                pids: measurementPIDs
            )

            if let allocationToken, let allocationControl {
                try await allocationControl.finish(
                    allocationToken,
                    startedUnixNanoseconds:
                        sampleStartedUnixNanoseconds,
                    endedUnixNanoseconds: ended
                )
                allocationCaptureFinished = true
            }
            measurementIntervals.append(
                RendererMeasurementInterval(
                    startedUnixNanoseconds:
                        sampleStartedUnixNanoseconds,
                    endedUnixNanoseconds: ended,
                    requiredAllocationPIDs:
                        try requiredAllocationPIDs(
                            control: allocationControl,
                            token: allocationToken
                        )
                )
            )
            cpu.append(
                max(
                    0,
                    afterResources.cpuMilliseconds
                        - beforeResources.cpuMilliseconds
                )
            )
            allocations.append(
                Double(
                    max(
                        0,
                        afterAllocator.blocks
                            - beforeAllocator.blocks
                    )
                )
            )
            growth.append(
                max(
                    0,
                    afterResources.physicalFootprintMiB
                        - beforeResources.physicalFootprintMiB
                )
            )
            sampleResourcesComplete =
                sampleResourcesComplete
                    && afterResources.measuredPIDCount
                        == measurementPIDs.count

            let resourceInspection = try await inspectDOM(in: webView)
            semanticParityPassed = semanticParityPassed
                && resourceInspection.nodes == completeParity
            elementKindsPassed = elementKindsPassed
                && resourceInspection.elementKindsPassed
                && resourceInspection.renderedPropertiesPassed
            try hideBenchmarkWindow(window)
        } catch {
            if window.isVisible {
                try? hideBenchmarkWindow(window)
            }
            if allocationCaptureFinished == false,
               let allocationToken,
               let allocationControl {
                try? await allocationControl.finish(
                    allocationToken,
                    startedUnixNanoseconds:
                        sampleStartedUnixNanoseconds,
                    endedUnixNanoseconds:
                        resourceEndedUnixNanoseconds
                            ?? benchmarkWallClockNanoseconds()
                )
            }
            throw error
        }

        guard let resourceEndedUnixNanoseconds else {
            throw BenchmarkFailure.message(
                "WebKit resource pass produced no display submission"
            )
        }
        let afterResourceHelperPIDs = Set(
            benchmarkWebKitHelperProcessIDs(webView)
        ).subtracting([getpid()])
        let afterResourceHelperIdentities =
            afterResourceHelperPIDs.compactMap {
                benchmarkProcessIdentity(pid: $0)
            }
        sampleResourcesComplete =
            sampleResourcesComplete
                && afterResourceHelperPIDs == beforeHelperPIDs
                && afterResourceHelperIdentities.count
                    == afterResourceHelperPIDs.count
                && afterResourceHelperIdentities.allSatisfy {
                    $0.observedAliveThroughUnixNanoseconds
                        >= resourceEndedUnixNanoseconds
                }
        for identity in afterResourceHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds
                    != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
        }
        helperPIDs.formUnion(afterResourceHelperPIDs)

        try await loadWebDOMState(
            webView,
            probe: probe,
            html: resetRepresentation,
            expectedParity: [],
            phase: "pre-peak hidden reset",
            requireAnimationFrame: false
        )
        guard window.isVisible == false,
              (try await inspectDOM(in: webView)).nodes.isEmpty else {
            throw BenchmarkFailure.message(
                "WebKit sample view did not reset before peak pass"
            )
        }

        let peakHelperPIDsByRole =
            benchmarkWebKitHelperProcessIDsByRole(webView)
        let peakHelperPIDs = Set(peakHelperPIDsByRole.values)
            .subtracting([getpid()])
        guard peakHelperPIDsByRole == beforeHelperPIDsByRole,
              peakHelperPIDs == beforeHelperPIDs else {
            throw BenchmarkFailure.message(
                "WebKit exact helper PID-role topology changed between "
                    + "resource and peak passes"
            )
        }

        let footprintSampler = ProcessFootprintSampler(
            processIDs: measurementPIDs
        )
        var footprintSamplerFinished = false
        defer {
            if footprintSamplerFinished == false {
                footprintSampler.cancel()
            }
        }
        let peakStartedUnixNanoseconds =
            benchmarkWallClockNanoseconds()
        footprintSampler.begin(at: peakStartedUnixNanoseconds)

        for state in completeVisualStates {
            try await loadWebDOMState(
                webView,
                probe: probe,
                html: state.html,
                expectedParity: state.expectedParity,
                phase: "peak \(state.phase)",
                requireAnimationFrame: false
            )
        }
        try submitWebViewForDisplay(webView, window: window)
        // Pair the pre-workload baseline with an unconditional post-workload
        // exact-PID sample before the peak interval is closed.
        footprintSampler.sampleNow()
        let peakEndedUnixNanoseconds =
            benchmarkWallClockNanoseconds()
        let footprintMeasurement = footprintSampler.finish(
            at: peakEndedUnixNanoseconds
        )
        footprintSamplerFinished = true

        peaks.append(
            footprintMeasurement.peakPhysicalFootprintMiB
        )
        let peakInspection = try await inspectDOM(in: webView)
        semanticParityPassed = semanticParityPassed
            && peakInspection.nodes == completeParity
        elementKindsPassed = elementKindsPassed
            && peakInspection.elementKindsPassed
            && peakInspection.renderedPropertiesPassed
        try hideBenchmarkWindow(window)

        let afterPeakHelperPIDs = Set(
            benchmarkWebKitHelperProcessIDs(webView)
        ).subtracting([getpid()])
        let afterPeakHelperIdentities = afterPeakHelperPIDs.compactMap {
            benchmarkProcessIdentity(pid: $0)
        }
        sampleResourcesComplete =
            sampleResourcesComplete
                && footprintMeasurement.sampleCount >= 2
                && footprintMeasurement.allTargetProcessesMeasured
                && afterPeakHelperPIDs == beforeHelperPIDs
                && afterPeakHelperIdentities.count
                    == afterPeakHelperPIDs.count
                && afterPeakHelperIdentities.allSatisfy {
                    $0.observedAliveThroughUnixNanoseconds
                        >= peakEndedUnixNanoseconds
                }
        for identity in afterPeakHelperIdentities {
            if let recorded = processIdentities[identity.pid],
               recorded.birthUnixNanoseconds
                    != identity.birthUnixNanoseconds {
                sampleResourcesComplete = false
            } else {
                processIdentities[identity.pid] = identity
            }
        }
        helperPIDs.formUnion(afterPeakHelperPIDs)

        guard let afterHostIdentity =
                benchmarkProcessIdentity(pid: getpid()),
              afterHostIdentity.birthUnixNanoseconds
                == hostIdentity.birthUnixNanoseconds,
              afterHostIdentity.observedAliveThroughUnixNanoseconds
                >= peakEndedUnixNanoseconds else {
            throw BenchmarkFailure.message(
                "WebKit host identity was not alive through resource "
                    + "and peak passes"
            )
        }
        processIdentities[getpid()] = afterHostIdentity
        let webContentIdentified =
            afterPeakHelperIdentities.contains {
                $0.pid == sampleWebContentPID
            }
        resourceAttributionComplete =
            resourceAttributionComplete
                && webContentIdentified
                && sampleResourcesComplete
    }

    let endedUnixNanoseconds = benchmarkWallClockNanoseconds()
    let attributedPIDs = Set([getpid()]).union(helperPIDs)
    resourceAttributionComplete =
        resourceAttributionComplete
            && Set(processIdentities.keys) == attributedPIDs
    let attribution = RendererProcessAttribution(
        candidate: "webkit",
        driverPID: driverPID,
        hostPID: getpid(),
        helperPIDs: helperPIDs.sorted(),
        processIdentities:
            processIdentities.values.sorted { $0.pid < $1.pid },
        startedUnixNanoseconds: startedUnixNanoseconds,
        endedUnixNanoseconds: endedUnixNanoseconds,
        measurementIntervals: measurementIntervals,
        helperPIDSource:
            "required WebContent PID from benchmark-only "
                + "_webProcessIdentifier; optional "
                + "_networkProcessIdentifier/_gpuProcessIdentifier values "
                + "included when available; every exact PID-role mapping "
                + "must remain unchanged through the separate resource "
                + "and peak-footprint passes"
    )
    let contentPresentationDetail =
        "\(contentPresentationPassCount)/\(iterations) samples passed; "
            + (
                firstContentPresentationFailure
                    ?? lastContentPresentationDetail
            )
    let requiredPixelCaptures = fullPaint ? iterations * 2 : 0
    return RendererCandidateResult(
        candidate: "webkit",
        firstPaint: first,
        completePaint: complete,
        cpuTime: cpu,
        hostLiveAllocationDelta: allocations,
        allocatedFootprintGrowthMiB: p50(growth),
        processFootprintPeak: peaks,
        renderedNodeCount: renderedNodeCount,
        presentationCompletions: presentationCompletions,
        representationBytes:
            firstRepresentation.count + completeRepresentation.count,
        semanticParityPassed: semanticParityPassed,
        elementKindsPassed: elementKindsPassed,
        resourceAttributionComplete: resourceAttributionComplete,
        captureAuthorization: captureAuthorization,
        pixelCaptureCompletions: pixelCaptureCompletions,
        contentPresentationPassed: contentPresentationPassed,
        contentPresentationDetail: contentPresentationDetail,
        paintCompletionMode: fullPaint
            ? "four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before the corresponding hidden WebKit load sequence and ends at the accepted ScreenCaptureKit frame displayTime; CPU/live-allocation/footprint-growth use a separate hidden-load plus display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only hidden-load plus display-submission pass"
            : "smoke-only first-state and complete-sequence timings end at separate offscreen WKSnapshot completions; CPU/live-allocation/footprint-growth and peak use separate hidden-load/display-submission passes, while no on-screen/compositor latency claim is made",
        attribution: attribution,
        succeeded: first.count == iterations
            && complete.count == iterations
            && cpu.count == iterations
            && allocations.count == iterations
            && peaks.count == iterations
            && renderedNodeCount == fixture.nodes.count
            && presentationCompletions == iterations * 2
            && semanticParityPassed
            && elementKindsPassed
            && resourceAttributionComplete
            && contentPresentationPassed
            && (!fullPaint || (
                captureAuthorization
                    && pixelCaptureCompletions == requiredPixelCaptures
            ))
    )
}
@MainActor
func startCandidateCleanupProbeDescendantIfRequested() throws -> pid_t? {
    guard let identityPath = ProcessInfo.processInfo.environment[
        "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH"
    ] else {
        return nil
    }
    var processID: pid_t = 0
    var arguments: [UnsafeMutablePointer<CChar>?] = [
        strdup("sleep"),
        strdup("300"),
        nil,
    ]
    defer {
        for argument in arguments where argument != nil {
            free(argument)
        }
    }
    let spawnStatus = arguments.withUnsafeMutableBufferPointer { buffer in
        posix_spawn(
            &processID,
            "/bin/sleep",
            nil,
            nil,
            buffer.baseAddress,
            environ
        )
    }
    guard spawnStatus == 0, processID > 0 else {
        throw BenchmarkFailure.message(
            "cleanup probe descendant spawn failed: errno \(spawnStatus)"
        )
    }
    var descendantHandedOff = false
    defer {
        if descendantHandedOff == false {
            stopCandidateCleanupProbeDescendant(processID)
        }
    }

    let deadline = Date().addingTimeInterval(2)
    var identity = benchmarkProcessIdentity(pid: processID)
    while identity == nil, Darwin.kill(processID, 0) == 0, Date() < deadline {
        _ = RunLoop.current.run(
            mode: .default,
            before: min(deadline, Date().addingTimeInterval(0.005))
        )
        identity = benchmarkProcessIdentity(pid: processID)
    }
    guard let identity,
          getpgid(processID) == getpid(),
          benchmarkProcessMatchesIdentity(
              pid: processID,
              birthUnixNanoseconds: identity.birthUnixNanoseconds
          ) else {
        throw BenchmarkFailure.message(
            "cleanup probe descendant did not join the candidate process group"
        )
    }
    let encodedIdentity = try JSONEncoder().encode(identity)
    if let observerPath = ProcessInfo.processInfo.environment[
        "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_OBSERVER_PATH"
    ] {
        try encodedIdentity.write(
            to: URL(fileURLWithPath: observerPath),
            options: .atomic
        )
    }
    try encodedIdentity.write(
        to: URL(fileURLWithPath: identityPath),
        options: .atomic
    )
    descendantHandedOff = true
    return processID
}

@MainActor
func stopCandidateCleanupProbeDescendant(_ processID: pid_t?) {
    guard let processID else { return }
    if Darwin.kill(processID, 0) == 0 {
        _ = Darwin.kill(processID, SIGKILL)
    }
    var status: Int32 = 0
    while waitpid(processID, &status, 0) == -1, errno == EINTR {}
}

func validateRendererCandidateAttribution(
    _ result: RendererCandidateResult,
    expectedCandidate: String,
    expectedDriverPID: Int32,
    expectedHostPID: Int32,
    expectedHostBirthUnixNanoseconds: UInt64,
    allocationTargetRole: String?
) throws {
    let attribution = result.attribution
    let attributedPIDs = Set([attribution.hostPID] + attribution.helperPIDs)
    let helperPIDs = Set(attribution.helperPIDs)
    let identityPIDs = Set(attribution.processIdentities.map(\.pid))
    let attributedHost = attribution.processIdentities.first {
        $0.pid == expectedHostPID
    }

    func allocationRequirementIsValid(
        _ interval: RendererMeasurementInterval
    ) -> Bool {
        switch allocationTargetRole {
        case nil:
            // Ordinary §31.1 resource measurements do not claim xctrace
            // allocation coverage.
            return interval.requiredAllocationPIDs.isEmpty
        case "host":
            return interval.requiredAllocationPIDs == [expectedHostPID]
        case "webcontent":
            return interval.requiredAllocationPIDs.count == 1
                && interval.requiredAllocationPIDs.allSatisfy(helperPIDs.contains)
        case "network", "gpu":
            // An absent optional helper is represented by an empty list only
            // after the version-2 capture handshake reported target_present=false.
            return interval.requiredAllocationPIDs.isEmpty
                || (
                    interval.requiredAllocationPIDs.count == 1
                        && interval.requiredAllocationPIDs.allSatisfy(
                            helperPIDs.contains
                        )
                )
        default:
            return false
        }
    }

    guard result.candidate == expectedCandidate,
          attribution.candidate == expectedCandidate,
          attribution.hostPID == expectedHostPID,
          attribution.driverPID == expectedDriverPID,
          attributedPIDs == identityPIDs,
          helperPIDs.contains(expectedHostPID) == false,
          Set(attribution.helperPIDs).count == attribution.helperPIDs.count,
          result.resourceAttributionComplete,
          attribution.processIdentities.allSatisfy({
              $0.birthUnixNanoseconds > 0
                  && $0.observedAliveThroughUnixNanoseconds
                      >= $0.birthUnixNanoseconds
          }),
          attribution.measurementIntervals.isEmpty == false,
          attribution.measurementIntervals.allSatisfy({
              $0.startedUnixNanoseconds >= attribution.startedUnixNanoseconds
                  && $0.startedUnixNanoseconds < $0.endedUnixNanoseconds
                  && $0.endedUnixNanoseconds <= attribution.endedUnixNanoseconds
                  && Set($0.requiredAllocationPIDs).count
                      == $0.requiredAllocationPIDs.count
                  && Set($0.requiredAllocationPIDs).isSubset(of: attributedPIDs)
                  && allocationRequirementIsValid($0)
          }),
          attributedHost?.birthUnixNanoseconds
            == expectedHostBirthUnixNanoseconds,
          attributedHost?.observedAliveThroughUnixNanoseconds
            ?? 0 >= (attribution.measurementIntervals.last?
                .endedUnixNanoseconds ?? UInt64.max) else {
        let mode = allocationTargetRole.map {
            "targeted allocation role \($0)"
        } ?? "ordinary non-allocation"
        throw BenchmarkFailure.message(
            "renderer candidate process attribution mismatch for \(mode) mode"
        )
    }
}

@MainActor
func runCandidateSubprocess(
    name: String,
    fixture: URL,
    profile: String
) throws -> RendererCandidateResult {
    let fileManager = FileManager.default
    let temporary = fileManager.temporaryDirectory
        .appendingPathComponent("srui-benchmark-\(name)-\(UUID().uuidString).json")
    let standardOutputURL = temporary.appendingPathExtension("stdout")
    let standardErrorURL = temporary.appendingPathExtension("stderr")
    guard fileManager.createFile(atPath: standardOutputURL.path, contents: nil),
          fileManager.createFile(atPath: standardErrorURL.path, contents: nil) else {
        throw BenchmarkFailure.message("renderer candidate log files could not be created")
    }
    let standardOutput = try FileHandle(forWritingTo: standardOutputURL)
    let standardError = try FileHandle(forWritingTo: standardErrorURL)
    defer {
        try? standardOutput.close()
        try? standardError.close()
        try? fileManager.removeItem(at: temporary)
        try? fileManager.removeItem(at: standardOutputURL)
        try? fileManager.removeItem(at: standardErrorURL)
    }

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
    process.standardOutput = standardOutput
    process.standardError = standardError
    try process.run()
    let processID = process.processIdentifier

    func waitForCandidateExit(until deadline: Date) -> Bool {
        while process.isRunning && Date() < deadline {
            _ = RunLoop.current.run(
                mode: .default,
                before: min(deadline, Date().addingTimeInterval(0.01))
            )
        }
        return process.isRunning == false
    }
    func forceStopAndReap(
        expectedIdentity: RendererProcessIdentity?
    ) -> String? {
        guard process.isRunning else { return nil }
        var signalled = false
        if let expectedIdentity {
            let signalDeadline = Date().addingTimeInterval(0.25)
            while process.isRunning, signalled == false, Date() < signalDeadline {
                signalled = benchmarkSignalLiveProcessGroup(
                    processID,
                    expectedBirthUnixNanoseconds:
                        expectedIdentity.birthUnixNanoseconds,
                    signal: SIGKILL
                )
                if signalled == false {
                    _ = RunLoop.current.run(
                        mode: .default,
                        before: min(
                            signalDeadline,
                            Date().addingTimeInterval(0.005)
                        )
                    )
                }
            }
        }
        // Never fall back to signaling the numeric PID: failed birth/group
        // validation may mean that PID now belongs to an unrelated process.
        let reaped = waitForCandidateExit(until: Date().addingTimeInterval(5))
        var failures = [String]()
        if reaped == false {
            failures.append("candidate was not reaped")
            if signalled == false {
                failures.append(
                    "candidate process-group ownership could not be validated"
                )
            }
        }
        return failures.isEmpty ? nil : failures.joined(separator: "; ")
    }

    var cleanupIdentity: RendererProcessIdentity?
    var launchedIdentity: RendererProcessIdentity?
    do {
        let identityDeadline = Date().addingTimeInterval(1)
        cleanupIdentity = benchmarkProcessIdentity(pid: processID)
        while cleanupIdentity == nil,
              process.isRunning,
              Date() < identityDeadline {
            _ = RunLoop.current.run(
                mode: .default,
                before: min(identityDeadline, Date().addingTimeInterval(0.005))
            )
            cleanupIdentity = benchmarkProcessIdentity(pid: processID)
        }
        guard let cleanupIdentity else {
            throw BenchmarkFailure.message(
                "renderer candidate \(name) cleanup identity was unavailable"
            )
        }
        if let identityPath = ProcessInfo.processInfo.environment[
            "SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH"
        ] {
            try JSONEncoder().encode(cleanupIdentity).write(
                to: URL(fileURLWithPath: identityPath),
                options: .atomic
            )
        }
        let forceIdentityFailure =
            ProcessInfo.processInfo.environment[
                "SRUI_BENCHMARK_FORCE_CANDIDATE_IDENTITY_FAILURE"
            ] == "1"
        if forceIdentityFailure,
           let descendantIdentityPath = ProcessInfo.processInfo.environment[
               "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH"
           ] {
            let descendantURL = URL(fileURLWithPath: descendantIdentityPath)
            let descendantDeadline = Date().addingTimeInterval(2)
            var descendantIdentity: RendererProcessIdentity?
            while descendantIdentity == nil,
                  process.isRunning,
                  Date() < descendantDeadline {
                descendantIdentity = try? JSONDecoder().decode(
                    RendererProcessIdentity.self,
                    from: Data(contentsOf: descendantURL)
                )
                if descendantIdentity == nil {
                    _ = RunLoop.current.run(
                        mode: .default,
                        before: min(
                            descendantDeadline,
                            Date().addingTimeInterval(0.005)
                        )
                    )
                }
            }
            guard let descendantIdentity,
                  getpgid(descendantIdentity.pid) == processID,
                  benchmarkProcessMatchesIdentity(
                      pid: descendantIdentity.pid,
                      birthUnixNanoseconds: descendantIdentity.birthUnixNanoseconds
                  ) else {
                throw BenchmarkFailure.message(
                    "renderer candidate \(name) cleanup probe descendant was unavailable"
                )
            }
        }
        launchedIdentity = forceIdentityFailure ? nil : cleanupIdentity
        guard let launchedIdentity else {
            throw BenchmarkFailure.message(
                "renderer candidate \(name) birth identity was unavailable"
            )
        }

        let deadline = Date().addingTimeInterval(profile == "full" ? 240 : 60)
        guard waitForCandidateExit(until: deadline) else {
            throw BenchmarkFailure.message("renderer candidate \(name) timed out")
        }

        try? standardOutput.synchronize()
        try? standardError.synchronize()
        let candidateStandardError = (try? String(
            contentsOf: standardErrorURL,
            encoding: .utf8
        )) ?? ""
        let candidateStandardOutput = (try? String(
            contentsOf: standardOutputURL,
            encoding: .utf8
        )) ?? ""
        let candidateDiagnostics = candidateStandardError.isEmpty
            ? candidateStandardOutput
            : candidateStandardError
        guard process.terminationReason == .exit, process.terminationStatus == 0 else {
            let diagnosticSuffix = candidateDiagnostics.isEmpty
                ? ""
                : ": \(candidateDiagnostics.suffix(4_000))"
            throw BenchmarkFailure.message(
                "renderer candidate \(name) exited with status \(process.terminationStatus)"
                    + diagnosticSuffix
            )
        }
        let result = try JSONDecoder().decode(
            RendererCandidateResult.self,
            from: Data(contentsOf: temporary)
        )
        try validateRendererCandidateAttribution(
            result,
            expectedCandidate: name,
            expectedDriverPID: getpid(),
            expectedHostPID: processID,
            expectedHostBirthUnixNanoseconds:
                launchedIdentity.birthUnixNanoseconds,
            allocationTargetRole: nil
        )
        return result
    } catch {
        if let cleanupFailure = forceStopAndReap(
            expectedIdentity: cleanupIdentity ?? launchedIdentity
        ) {
            throw BenchmarkFailure.message(
                "\(error); candidate cleanup failed: \(cleanupFailure)"
            )
        }
        throw error
    }
}

struct LocalRendererResult {
    let section: Section
    let attributions: [RendererProcessAttribution]
}
@MainActor
func localRenderer(
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
        sampleCounts: [
            "macos.srui.render": srui.firstPaint.count,
            "macos.webkit.render": web.firstPaint.count,
        ],
        metrics: [
            metric(nativeFirstName, p50(srui.firstPaint), id: "srui.first_paint"),
            metric(nativeFirstName, percentile(srui.firstPaint, 0.95), "ms", "p95", id: "srui.first_paint"),
            metric(nativeCompleteName, p50(srui.completePaint), id: "srui.complete_paint"),
            metric(nativeCompleteName, percentile(srui.completePaint, 0.95), "ms", "p95", id: "srui.complete_paint"),
            metric(nativeCompleteName, percentile(srui.completePaint, 0.99), "ms", "p99", id: "srui.complete_paint"),
            metric("SRUI candidate process CPU time", p50(srui.cpuTime), id: "srui.cpu"),
            metric("SRUI candidate process CPU time", percentile(srui.cpuTime, 0.95), "ms", "p95", id: "srui.cpu"),
            metric("SRUI host retained allocation delta", p50(srui.hostLiveAllocationDelta), "allocations", id: "srui.host_retained_allocations"),
            metric("SRUI host allocated footprint growth", srui.allocatedFootprintGrowthMiB, "MiB", "p50", id: "srui.process_footprint_growth"),
            metric("SRUI maximum concurrently sampled process footprint", srui.processFootprintPeak.max() ?? -1, "MiB", "max", id: "srui.process_footprint_peak"),
            metric(webFirstName, p50(web.firstPaint), id: "webkit.first_paint"),
            metric(webFirstName, percentile(web.firstPaint, 0.95), "ms", "p95", id: "webkit.first_paint"),
            metric(webCompleteName, p50(web.completePaint), id: "webkit.complete_paint"),
            metric(webCompleteName, percentile(web.completePaint, 0.95), "ms", "p95", id: "webkit.complete_paint"),
            metric("WKWebView host plus attributed helper CPU time", p50(web.cpuTime), id: "webkit.cpu"),
            metric("WKWebView host retained allocation delta", p50(web.hostLiveAllocationDelta), "allocations", id: "webkit.host_retained_allocations"),
            metric("WKWebView host plus helpers allocated footprint growth", web.allocatedFootprintGrowthMiB, "MiB", "p50", id: "webkit.process_footprint_growth"),
            metric("WKWebView maximum concurrently sampled host-plus-helper footprint", web.processFootprintPeak.max() ?? -1, "MiB", "max", id: "webkit.process_footprint_peak"),
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
                    ? "candidate production state reaches a verified composited target-pixel frame"
                    : "candidate content draw completes in the offscreen raster fallback",
                passed: srui.succeeded && web.succeeded,
                detail: "\(srui.presentationCompletions) native and \(web.presentationCompletions) WebKit completions; native content: \(srui.contentPresentationDetail); WebKit content: \(web.contentPresentationDetail); \(srui.paintCompletionMode); \(web.paintCompletionMode)"
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
                ? "Every accepted full-paint state requires an exact visible CGWindow, exact client/target geometry, unobscured z-order, and authorized nonblank/nonuniform ScreenCaptureKit pixels from one complete frame. Latency is action-start Mach time through that accepted frame's SCStream displayTime; callback receipt and pixel hashing are verifier metadata, not the presentation timestamp. The two reported timed states per logical sample produced native=\(srui.pixelCaptureCompletions) and WebKit=\(web.pixelCaptureCompletions) verified captures; capture_authorization=\(captureAuthorization). Warm/resource/peak passes are not counted in that metric. No permission request is issued."
                : "Smoke deliberately uses named offscreen AppKit bitmap and WKSnapshot fallbacks; it validates state and raster completion but makes no WindowServer, visibility, or composited-pixel claim.",
            "Each native logical sample uses four separately warmed/reset renderer instances: first-state visual latency, complete two-transaction visual latency, CPU/live-allocation/footprint-growth through production display submission, and sampler-only peak footprint. Each WebKit sample resets one warmed view between the same four disjoint workloads while preserving exact helper PID identities. Neither resource pass creates ScreenCaptureKit buffers, and the footprint sampler never runs in the CPU/allocation pass.",
            "Native first timing decodes and applies revision 0→1 through production initial attach. Native complete timing decodes/applies 0→1 and then applies 1→2 through production incremental apply. WebKit uses structurally equivalent first and complete DOM states. The measured instance is checked immediately after each accepted presentation for exact semantic/control or DOM state. Full mode additionally requires nonblank, nonuniform, equal-geometry client-content fingerprints that differ between first and complete states.",
            "Each candidate process group is birth-identity verified. WebKit CPU, footprint growth, and peak footprint aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs; retained malloc block counts are explicitly host-only. Peak footprint is the maximum simultaneous current-footprint sample at 1 ms cadence strictly inside its separate representative pass, not a sum of per-process lifetime maxima.",
            "The warmed WKWebView candidate is a comparison control only; it is not the production SRUI renderer and its timing does not describe SRUI's native AppKit rendering path.",
        ]
    )
    return LocalRendererResult(
        section: section,
        attributions: [srui.attribution, web.attribution]
    )
}
