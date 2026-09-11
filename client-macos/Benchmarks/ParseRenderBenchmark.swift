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
    let properties: [String: FixtureValue]
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

func encodedProperties(_ properties: [String: FixtureValue]) throws -> String {
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
        case "Menu":
            let options = (properties["items"]?.stringListValue ?? []).map {
                "<option>\(escapedHTML($0))</option>"
            }.joined()
            return "<select\(metadata)\(attribute("aria-label", label))>\(options)</select>\(descendants)"
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
        case "Menu":
            guard let menu = handle.view as? NSPopUpButton,
                  menu.itemTitles == (properties["items"]?.stringListValue ?? []) else {
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
    var continuation: CheckedContinuation<Value, any Error>?
    var timeoutTask: Task<Void, Never>?

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

    func finish(_ result: Result<Value, any Error>) {
        guard let continuation else { return }
        self.continuation = nil
        timeoutTask?.cancel()
        timeoutTask = nil
        continuation.resume(with: result)
    }
}

@MainActor
final class AnimationFrameProbe: NSObject, WKScriptMessageHandler {
    weak var controller: WKUserContentController?
    let name: String
    let gate: TimedContinuation<Void>

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
        TextArea: "TEXTAREA", Button: "BUTTON", Menu: "SELECT", Separator: "HR"
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
          case "Menu":
            return JSON.stringify(Array.from(element.options, (option) => option.text))
                === JSON.stringify(properties.items ?? [])
              && (properties.label === undefined
                || element.getAttribute("aria-label") === properties.label);
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
let sruiHostAllocationMeasurementScope =
    "default malloc zone in the SRUI renderer host process only; "
        + "signed after-minus-before net live state, not cumulative "
        + "allocation events"
let webKitHostAllocationMeasurementScope =
    "default malloc zone in the WebKit comparison host process only; "
        + "excludes WebContent, Network, and GPU helper processes; "
        + "signed after-minus-before net live state, not cumulative "
        + "allocation events"

struct RendererCandidateResult: Codable {
    let candidate: String
    let expectedSampleCount: Int
    let firstPaint: [Double]
    let completePaint: [Double]
    let cpuTime: [Double]
    let hostNetLiveAllocationBlockDelta: [Double]
    let hostNetLiveAllocationByteDelta: [Double]
    let hostAllocationMeasurementScope: String
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
            detail: "non-compositor same-instance semantic/control validation plus separate offscreen raster or WKSnapshot completions; no WindowServer, compositor, visibility, or captured-content claim"
        )
    }
    guard first.crossedDisplayRefresh,
          complete.crossedDisplayRefresh,
          first.captureAuthorization,
          complete.captureAuthorization,
          first.pixelCaptureVerified,
          complete.pixelCaptureVerified,
          let firstEvidence = first.compositedContentEvidence,
          let completeEvidence = complete.compositedContentEvidence,
          let firstNormalization =
              first.compositedContentNormalization,
          let completeNormalization =
              complete.compositedContentNormalization else {
        return ProgressiveContentEvidenceCheck(
            passed: false,
            detail: "full-paint observation lacked authorized normalized composited client-content evidence; first provenance=\(first.visibilityProvenance), complete provenance=\(complete.visibilityProvenance)"
        )
    }
    let sameGeometry =
        firstEvidence.pixelWidth == completeEvidence.pixelWidth
            && firstEvidence.pixelHeight == completeEvidence.pixelHeight
            && firstEvidence.unmaskedPixelCount
                == completeEvidence.unmaskedPixelCount
            && firstNormalization == completeNormalization
    let usefulContent =
        firstEvidence.hasNonblankContent
            && firstEvidence.hasNonuniformContent
            && completeEvidence.hasNonblankContent
            && completeEvidence.hasNonuniformContent
    let contentDelta = sameGeometry
        ? benchmarkCompositedDeltaEvidence(
            completeEvidence,
            firstEvidence,
            normalization: completeNormalization,
            channelTolerance: benchmarkStreamCaptureChannelTolerance
        ) : nil
    let distinctContent =
        contentDelta.map {
            $0.materiallyDifferentPixelCount
                >= $0.requiredMaterialPixelCount
        } ?? false
    let passed = sameGeometry && usefulContent && distinctContent
    return ProgressiveContentEvidenceCheck(
        passed: passed,
        detail: "full composited client-content proof: first=\(String(firstEvidence.normalizedFingerprintSHA256.prefix(16))) complete=\(String(completeEvidence.normalizedFingerprintSHA256.prefix(16))) dimensions=\(firstEvidence.pixelWidth)x\(firstEvidence.pixelHeight)/\(completeEvidence.pixelWidth)x\(completeEvidence.pixelHeight) unmasked=\(firstEvidence.unmaskedPixelCount)/\(completeEvidence.unmaskedPixelCount) quantized-colors=\(firstEvidence.distinctQuantizedColorCount)/\(completeEvidence.distinctQuantizedColorCount) non-dominant=\(firstEvidence.nonDominantPixelCount)/\(completeEvidence.nonDominantPixelCount) nonblank-and-nonuniform=\(usefulContent) material-pixels=\(contentDelta?.materiallyDifferentPixelCount ?? -1)/\(contentDelta?.requiredMaterialPixelCount ?? 8) max-channel-delta=\(contentDelta?.maximumChannelDelta ?? -1)/255 tolerance=\(benchmarkStreamCaptureChannelTolerance)/255 materially-distinct=\(distinctContent) same-geometry-and-normalization=\(sameGeometry) provenance=\(first.visibilityProvenance)/\(complete.visibilityProvenance)"
    )
}
