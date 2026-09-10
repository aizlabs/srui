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

func terminalFullPlainText(_ snapshot: TerminalSnapshot) -> String {
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

struct TerminalProductionSample {
    let decodeVisibleMilliseconds: Double
    let drawOnlyMilliseconds: Double
    let framedBytes: Int
    let contentDigest: String
    let renderedBitmapDigest: String?
    let viewSize: NSSize
    let gridColumns: Int
    let gridRows: Int
    let offsetExact: Bool
    let contentExact: Bool
    let drawCompleted: Bool
    let productionPathExact: Bool
    let visibilityProvenance: String
    let compositedContentFingerprint: String?
    let compositedContentVerified: Bool
}

struct StandaloneTerminalDisplaySample {
    let decodeVisibleMilliseconds: Double
    let drawOnlyMilliseconds: Double
    let contentDigest: String
    let renderedBitmapDigest: String?
    let payloadDigest: String
    let offsetExact: Bool
    let contentExact: Bool
    let drawCompleted: Bool
    let visibilityProvenance: String
    let compositedContentFingerprint: String?
    let compositedContentVerified: Bool
}

@MainActor
private func installTerminalDrawProbe(in window: NSWindow) throws -> BenchmarkDrawCompletionProbe {
    guard let contentView = window.contentView else {
        throw BenchmarkFailure.message("terminal benchmark window has no content view")
    }
    let probe = BenchmarkDrawCompletionProbe(frame: contentView.bounds)
    probe.autoresizingMask = [.width, .height]
    contentView.addSubview(probe, positioned: .above, relativeTo: nil)
    return probe
}

@MainActor
func standaloneTerminalDisplaySample(
    payload: Data,
    expectedPlainText: String,
    expectedContentDigest: String,
    streamID: NodeId,
    viewSize: NSSize,
    gridColumns: Int,
    gridRows: Int,
    fullPaint: Bool
) async throws -> StandaloneTerminalDisplaySample {
    guard viewSize.width > 0, viewSize.height > 0,
          gridColumns > 0, gridRows > 0 else {
        throw BenchmarkFailure.message(
            "embedded terminal did not provide valid standalone baseline geometry"
        )
    }

    let terminalSession = TerminalSession()
    await terminalSession.resize(
        streamID: streamID,
        columns: gridColumns,
        rows: gridRows
    )
    guard let initialSnapshot = await terminalSession.snapshot(for: streamID) else {
        throw BenchmarkFailure.message(
            "standalone TerminalSession did not establish the requested grid"
        )
    }
    let snapshots = await terminalSession.snapshots(for: streamID)
    let terminalView = TerminalView(nodeID: streamID)
    let rootView = NSView(frame: NSRect(origin: .zero, size: viewSize))
    terminalView.frame = rootView.bounds
    terminalView.autoresizingMask = [.width, .height]
    rootView.addSubview(terminalView)

    let window = NSWindow(
        contentRect: NSRect(origin: .zero, size: viewSize),
        styleMask: [.titled, .closable, .resizable],
        backing: .buffered,
        defer: false
    )
    window.isReleasedWhenClosed = false
    window.contentView = rootView
    window.center()
    terminalView.snapshotSubscriptionTask = Task { @MainActor [weak terminalView] in
        for await snapshot in snapshots {
            guard let terminalView else { break }
            terminalView.apply(snapshot)
        }
    }
    defer {
        terminalView.snapshotSubscriptionTask?.cancel()
        window.contentView = nil
        window.close()
    }

    try await waitUntil {
        (terminalView.accessibilityValue() as? String) == initialSnapshot.plainText()
    }
    let blankBitmap = renderedBitmapSignature(terminalView)

    let decodeStarted: ContinuousClock.Instant
    let drawStarted: ContinuousClock.Instant
    let visible: Bool
    let visibleAt: ContinuousClock.Instant
    let decodeVisibleMilliseconds: Double
    let drawOnlyMilliseconds: Double
    let visibilityProvenance: String
    let compositedContentFingerprint: String?
    let compositedContentVerified: Bool
    if fullPaint {
        var contentAppliedAt: ContinuousClock.Instant?
        var contentAppliedMachTicks: UInt64?
        let measurement = try await benchmarkMeasurePassiveCompositedChange(
            window,
            targetView: terminalView
        ) {
            _ = try await terminalSession.applyData(
                streamID: streamID,
                byteOffset: 0,
                data: payload
            )
            try await waitUntil {
                guard let snapshot = await terminalSession.snapshot(
                    for: streamID
                ),
                      snapshot.nextOffset == UInt64(payload.count) else {
                    return false
                }
                return (terminalView.accessibilityValue() as? String)
                    == snapshot.plainText()
            }
            contentAppliedMachTicks = mach_absolute_time()
            contentAppliedAt = clock.now
        }
        guard let contentAppliedAt, let contentAppliedMachTicks else {
            throw BenchmarkFailure.message(
                "standalone terminal action did not reach decoded view state"
            )
        }
        decodeStarted = measurement.actionStartedAt
        drawStarted = contentAppliedAt
        decodeVisibleMilliseconds =
            measurement.presentationLatencyMilliseconds
        drawOnlyMilliseconds = try benchmarkMachElapsedMilliseconds(
            from: contentAppliedMachTicks,
            to: measurement.acceptedDisplayMachTicks
        )
        let observation = measurement.observation
        visible = observation.crossedDisplayRefresh
        visibleAt = observation.presentedAt
        visibilityProvenance = observation.visibilityProvenance
        compositedContentFingerprint = observation.compositedContentEvidence?
            .normalizedFingerprintSHA256
        compositedContentVerified =
            observation.compositedContentEvidence?.hasNonblankContent == true
            && observation.compositedContentEvidence?
                .hasNonuniformContent == true
    } else {
        decodeStarted = clock.now
        _ = try await terminalSession.applyData(
            streamID: streamID,
            byteOffset: 0,
            data: payload
        )
        try await waitUntil {
            guard let snapshot = await terminalSession.snapshot(for: streamID),
                  snapshot.nextOffset == UInt64(payload.count) else {
                return false
            }
            return (terminalView.accessibilityValue() as? String)
                == snapshot.plainText()
        }
        drawStarted = clock.now
        visible = rasterize(terminalView)
        visibleAt = clock.now
        decodeVisibleMilliseconds = milliseconds(
            decodeStarted.duration(to: visibleAt)
        )
        drawOnlyMilliseconds = milliseconds(
            drawStarted.duration(to: visibleAt)
        )
        visibilityProvenance = "offscreen_bitmap_smoke"
        compositedContentFingerprint = nil
        compositedContentVerified = false
    }

    guard let snapshot = await terminalSession.snapshot(for: streamID) else {
        throw BenchmarkFailure.message(
            "standalone TerminalSession lost its stream after delivery"
        )
    }
    let fullPlainText = terminalFullPlainText(snapshot)
    let contentDigest = digestHex(Data(fullPlainText.utf8))
    let renderedBitmap = renderedBitmapSignature(terminalView)
    let contentDrawn = blankBitmap != nil
        && renderedBitmap != nil
        && blankBitmap != renderedBitmap

    return StandaloneTerminalDisplaySample(
        decodeVisibleMilliseconds: decodeVisibleMilliseconds,
        drawOnlyMilliseconds: drawOnlyMilliseconds,
        contentDigest: contentDigest,
        renderedBitmapDigest: renderedBitmap.map(digestHex),
        payloadDigest: digestHex(payload),
        offsetExact: snapshot.nextOffset == UInt64(payload.count),
        contentExact: terminalFullPlainText(initialSnapshot).isEmpty
            && fullPlainText == expectedPlainText
            && contentDigest == expectedContentDigest
            && (terminalView.accessibilityValue() as? String)
                == snapshot.plainText(),
        drawCompleted: visible
            && contentDrawn
            && (fullPaint == false || compositedContentVerified),
        visibilityProvenance: visibilityProvenance,
        compositedContentFingerprint: compositedContentFingerprint,
        compositedContentVerified: compositedContentVerified
    )
}

@MainActor
func productionTerminalSample(
    payload: Data,
    expectedPlainText: String,
    expectedContentDigest: String,
    streamID: NodeId,
    namespaceID: UInt32,
    sampleIndex: Int,
    fullPaint: Bool
) async throws -> TerminalProductionSample {
    let renderer = AppKitRenderer()
    let transport = BenchmarkTransport()
    let sessionID = "terminal-benchmark-\(sampleIndex)"
    let terminalType = terminalTypeRef(namespaceID: namespaceID)
    let controller = SessionController(
        transport: transport,
        renderer: renderer,
        clientCapabilities: [Profile.standardWidgetsV1, Profile.terminalV1],
        requiredServerProfiles: [Profile.terminalV1]
    )
    controller.attachRenderer(renderer)

    do {
        try await controller.start()
        let outboundFrames = await transport.framesSent()
        let helloOfferedTerminal = try outboundFrames.contains { frame in
            let message = try SRUIFraming.decodeFramed(SRUIMessage.self, from: frame.data)
            guard case .clientHello(let hello)? = message.msg else { return false }
            return frame.logicalClass == .control
                && hello.profiles.contains(Profile.terminalV1.description)
        }

        try await transport.injectFromServer(
            try framed(
                terminalWelcomeMessage(
                    sessionID: sessionID,
                    namespaceID: namespaceID
                )
            )
        )
        try await waitUntil {
            controller.isHandshakeComplete
                && controller.negotiatedCapabilities
                    == [Profile.standardWidgetsV1, Profile.terminalV1]
                && renderer.controlFactory.extensionKind(for: terminalType) == .terminal
        }

        let mount = Transaction(
            baseRevision: .initial,
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: streamID,
                    nodeType: terminalType,
                    parentID: NodeId(1)
                ),
            ]
        )
        try await transport.injectFromServer(
            try framed(transactionMessage(mount))
        )
        try await waitForRevision(Revision(1), controller: controller)
        try await waitUntil {
            renderer.registry.view(for: streamID) is TerminalView
                && renderer.registry.surfaceHandles.first?.window != nil
        }

        guard let terminalView = renderer.registry.view(for: streamID) as? TerminalView,
              let surfaceWindow = renderer.registry.surfaceHandles.first?.window else {
            throw BenchmarkFailure.message(
                "framed terminal mount did not create a renderer-owned TerminalView and surface"
            )
        }
        let terminalSession = renderer.terminalSession
        try await waitUntil {
            await terminalSession.snapshot(for: streamID) != nil
        }
        guard let initialSnapshot = await terminalSession.snapshot(for: streamID) else {
            throw BenchmarkFailure.message(
                "renderer-owned TerminalSession did not establish the mounted stream"
            )
        }
        let startedFresh = initialSnapshot.nextOffset == 0
            && terminalFullPlainText(initialSnapshot).isEmpty

        let blankBitmap = renderedBitmapSignature(terminalView)

        var terminalData = SRUITerminalData()
        terminalData.streamID = streamID.value
        terminalData.byteOffset = 0
        terminalData.data = payload
        var terminalMessage = SRUIMessage()
        terminalMessage.terminalData = terminalData
        let terminalFrame = try framed(terminalMessage)
        let decodedFrame = try SRUIFraming.decodeFramed(
            SRUIMessage.self,
            from: terminalFrame
        )
        let framedPayloadExact: Bool
        if case .terminalData(let decoded)? = decodedFrame.msg {
            framedPayloadExact = decoded.streamID == streamID.value
                && decoded.byteOffset == 0
                && decoded.data == payload
        } else {
            framedPayloadExact = false
        }

        let transportBefore = await transport.snapshot()
        let decodeStarted: ContinuousClock.Instant
        let drawStarted: ContinuousClock.Instant
        let visible: Bool
        let visibleAt: ContinuousClock.Instant
        let decodeVisibleMilliseconds: Double
        let drawOnlyMilliseconds: Double
        let visibilityProvenance: String
        let compositedContentFingerprint: String?
        let compositedContentVerified: Bool
        if fullPaint {
            var contentAppliedAt: ContinuousClock.Instant?
            var contentAppliedMachTicks: UInt64?
            let measurement = try await benchmarkMeasurePassiveCompositedChange(
                surfaceWindow,
                targetView: terminalView
            ) {
                try await transport.injectFromServer(terminalFrame)
                try await waitUntil {
                    guard let snapshot = await terminalSession.snapshot(
                        for: streamID
                    ),
                          snapshot.nextOffset == UInt64(payload.count) else {
                        return false
                    }
                    return (terminalView.accessibilityValue() as? String)
                        == snapshot.plainText()
                }
                contentAppliedMachTicks = mach_absolute_time()
                contentAppliedAt = clock.now
            }
            guard let contentAppliedAt, let contentAppliedMachTicks else {
                throw BenchmarkFailure.message(
                    "embedded terminal action did not reach decoded view state"
                )
            }
            decodeStarted = measurement.actionStartedAt
            drawStarted = contentAppliedAt
            decodeVisibleMilliseconds =
                measurement.presentationLatencyMilliseconds
            drawOnlyMilliseconds = try benchmarkMachElapsedMilliseconds(
                from: contentAppliedMachTicks,
                to: measurement.acceptedDisplayMachTicks
            )
            let observation = measurement.observation
            visible = observation.crossedDisplayRefresh
            visibleAt = observation.presentedAt
            visibilityProvenance = observation.visibilityProvenance
            compositedContentFingerprint = observation
                .compositedContentEvidence?.normalizedFingerprintSHA256
            compositedContentVerified =
                observation.compositedContentEvidence?
                    .hasNonblankContent == true
                && observation.compositedContentEvidence?
                    .hasNonuniformContent == true
        } else {
            decodeStarted = clock.now
            try await transport.injectFromServer(terminalFrame)
            try await waitUntil {
                guard let snapshot = await terminalSession.snapshot(
                    for: streamID
                ),
                      snapshot.nextOffset == UInt64(payload.count) else {
                    return false
                }
                return (terminalView.accessibilityValue() as? String)
                    == snapshot.plainText()
            }
            drawStarted = clock.now
            visible = rasterize(terminalView)
            visibleAt = clock.now
            decodeVisibleMilliseconds = milliseconds(
                decodeStarted.duration(to: visibleAt)
            )
            drawOnlyMilliseconds = milliseconds(
                drawStarted.duration(to: visibleAt)
            )
            visibilityProvenance = "offscreen_bitmap_smoke"
            compositedContentFingerprint = nil
            compositedContentVerified = false
        }
        let transportAfter = await transport.snapshot()

        guard let snapshot = await terminalSession.snapshot(for: streamID) else {
            throw BenchmarkFailure.message(
                "renderer-owned TerminalSession lost the mounted stream after delivery"
            )
        }
        let fullPlainText = terminalFullPlainText(snapshot)
        let contentDigest = digestHex(Data(fullPlainText.utf8))
        let renderedBitmap = renderedBitmapSignature(terminalView)
        let contentDrawn = blankBitmap != nil
            && renderedBitmap != nil
            && blankBitmap != renderedBitmap
        let viewContentExact = (terminalView.accessibilityValue() as? String)
            == snapshot.plainText()
        let mountedNodeExact = controller.applier.currentSnapshot.store
            .getNode(streamID)?.nodeType == terminalType
        let terminalInboundMessages = transportAfter.inboundMessages
            - transportBefore.inboundMessages
        let terminalInboundBytes = transportAfter.inboundBytes
            - transportBefore.inboundBytes
        let productionPathExact = helloOfferedTerminal
            && controller.isHandshakeComplete
            && renderer.controlFactory.extensionKind(for: terminalType) == .terminal
            && terminalView.nodeID == streamID
            && mountedNodeExact
            && framedPayloadExact
            && terminalInboundMessages == 1
            && terminalInboundBytes == terminalFrame.count
            && viewContentExact

        let sample = TerminalProductionSample(
            decodeVisibleMilliseconds: decodeVisibleMilliseconds,
            drawOnlyMilliseconds: drawOnlyMilliseconds,
            framedBytes: terminalFrame.count,
            contentDigest: contentDigest,
            renderedBitmapDigest: renderedBitmap.map(digestHex),
            viewSize: terminalView.bounds.size,
            gridColumns: snapshot.cells.first?.count ?? 0,
            gridRows: snapshot.cells.count,
            offsetExact: snapshot.nextOffset == UInt64(payload.count),
            contentExact: startedFresh
                && fullPlainText == expectedPlainText
                && contentDigest == expectedContentDigest,
            drawCompleted: visible
                && contentDrawn
                && (fullPaint == false || compositedContentVerified),
            productionPathExact: productionPathExact,
            visibilityProvenance: visibilityProvenance,
            compositedContentFingerprint: compositedContentFingerprint,
            compositedContentVerified: compositedContentVerified
        )
        await controller.stop()
        closeRenderer(renderer)
        return sample
    } catch {
        await controller.stop()
        closeRenderer(renderer)
        throw error
    }
}

@MainActor
func terminal(iterations: Int, fullPaint: Bool) async throws -> Section {
    let line = Data("\u{1b}[32mbenchmark output\u{1b}[0m\r\n".utf8)
    var payload = Data()
    payload.reserveCapacity(line.count * 256)
    for _ in 0..<256 { payload.append(line) }
    guard payload.count == 6_912 else {
        throw BenchmarkFailure.message(
            "terminal benchmark payload must be exactly 6,912 bytes, got \(payload.count)"
        )
    }
    let payloadDigest = digestHex(payload)
    let expectedPlainText = Array(
        repeating: "benchmark output",
        count: 256
    ).joined(separator: "\n")
    let expectedContentDigest = digestHex(Data(expectedPlainText.utf8))
    var observedEmbeddedContentDigests = Set<String>()
    var observedStandaloneContentDigests = Set<String>()
    var observedEmbeddedBitmapDigests = Set<String>()
    var observedStandaloneBitmapDigests = Set<String>()
    var observedEmbeddedVisibilityProvenances = Set<String>()
    var observedStandaloneVisibilityProvenances = Set<String>()
    var observedEmbeddedCompositedFingerprints = Set<String>()
    var observedStandaloneCompositedFingerprints = Set<String>()
    var observedFramedBytes = Set<Int>()

    let streamID = NodeId(14)
    let namespaceID: UInt32 = 31
    var embeddedDecodeVisibleSamples = [Double]()
    var embeddedDrawOnlySamples = [Double]()
    var standaloneDecodeVisibleSamples = [Double]()
    var standaloneDrawOnlySamples = [Double]()
    var embeddedOffsetsExact = true
    var standaloneOffsetsExact = true
    var freshState = true
    var productionPathExact = true
    var displayContentExact = true
    var displayRasterExact = true
    var compositedContentEvidenceExact = true
    var embeddedDrawCompletions = 0
    var standaloneDrawCompletions = 0

    for sampleIndex in 0..<iterations {
        let embedded = try await productionTerminalSample(
            payload: payload,
            expectedPlainText: expectedPlainText,
            expectedContentDigest: expectedContentDigest,
            streamID: streamID,
            namespaceID: namespaceID,
            sampleIndex: sampleIndex,
            fullPaint: fullPaint
        )
        let standalone = try await standaloneTerminalDisplaySample(
            payload: payload,
            expectedPlainText: expectedPlainText,
            expectedContentDigest: expectedContentDigest,
            streamID: streamID,
            viewSize: embedded.viewSize,
            gridColumns: embedded.gridColumns,
            gridRows: embedded.gridRows,
            fullPaint: fullPaint
        )

        embeddedDecodeVisibleSamples.append(embedded.decodeVisibleMilliseconds)
        embeddedDrawOnlySamples.append(embedded.drawOnlyMilliseconds)
        standaloneDecodeVisibleSamples.append(standalone.decodeVisibleMilliseconds)
        standaloneDrawOnlySamples.append(standalone.drawOnlyMilliseconds)
        observedEmbeddedContentDigests.insert(embedded.contentDigest)
        observedStandaloneContentDigests.insert(standalone.contentDigest)
        if let digest = embedded.renderedBitmapDigest {
            observedEmbeddedBitmapDigests.insert(digest)
        }
        if let digest = standalone.renderedBitmapDigest {
            observedStandaloneBitmapDigests.insert(digest)
        }
        observedFramedBytes.insert(embedded.framedBytes)
        observedEmbeddedVisibilityProvenances.insert(
            embedded.visibilityProvenance
        )
        observedStandaloneVisibilityProvenances.insert(
            standalone.visibilityProvenance
        )
        if let fingerprint = embedded.compositedContentFingerprint {
            observedEmbeddedCompositedFingerprints.insert(fingerprint)
        }
        if let fingerprint = standalone.compositedContentFingerprint {
            observedStandaloneCompositedFingerprints.insert(fingerprint)
        }
        compositedContentEvidenceExact = compositedContentEvidenceExact
            && (
                fullPaint == false
                    || (
                        embedded.compositedContentVerified
                            && standalone.compositedContentVerified
                            && embedded.compositedContentFingerprint != nil
                            && standalone.compositedContentFingerprint != nil
                    )
            )
        embeddedOffsetsExact = embeddedOffsetsExact && embedded.offsetExact
        standaloneOffsetsExact = standaloneOffsetsExact && standalone.offsetExact
        freshState = freshState && embedded.contentExact && standalone.contentExact
        productionPathExact = productionPathExact && embedded.productionPathExact
        displayContentExact = displayContentExact
            && standalone.payloadDigest == payloadDigest
            && embedded.contentDigest == standalone.contentDigest
            && standalone.contentDigest == expectedContentDigest
        displayRasterExact = displayRasterExact
            && embedded.renderedBitmapDigest != nil
            && embedded.renderedBitmapDigest == standalone.renderedBitmapDigest
        if embedded.drawCompleted {
            embeddedDrawCompletions += 1
        }
        if standalone.drawCompleted {
            standaloneDrawCompletions += 1
        }
    }

    guard observedFramedBytes.count == 1,
          let exactFramedBytes = observedFramedBytes.first else {
        throw BenchmarkFailure.message(
            "terminal samples produced inconsistent framed envelope byte counts: \(observedFramedBytes.sorted())"
        )
    }

    let embeddedDecodeP50 = p50(embeddedDecodeVisibleSamples)
    let embeddedDrawP50 = p50(embeddedDrawOnlySamples)
    let standaloneDecodeP50 = p50(standaloneDecodeVisibleSamples)
    let standaloneDrawP50 = p50(standaloneDrawOnlySamples)

    return Section(
        id: "31.6",
        name: "Terminal",
        sampleCounts: [
            "macos.embedded_display": embeddedDecodeVisibleSamples.count,
            "macos.standalone_display": standaloneDecodeVisibleSamples.count,
        ],
        metrics: [
            metric("embedded SRUI Terminal decode-to-visible", embeddedDecodeP50, id: "client_terminal.decode_visible"),
            metric("embedded SRUI Terminal decode-to-visible", percentile(embeddedDecodeVisibleSamples, 0.95), "ms", "p95", id: "client_terminal.decode_visible"),
            metric("embedded SRUI Terminal decode-to-visible", percentile(embeddedDecodeVisibleSamples, 0.99), "ms", "p99", id: "client_terminal.decode_visible"),
            metric("embedded SRUI Terminal draw-only", embeddedDrawP50, id: "client_terminal.draw_only"),
            metric("embedded SRUI Terminal draw-only", percentile(embeddedDrawOnlySamples, 0.95), "ms", "p95", id: "client_terminal.draw_only"),
            metric("embedded SRUI Terminal draw-only", percentile(embeddedDrawOnlySamples, 0.99), "ms", "p99", id: "client_terminal.draw_only"),
            metric("standalone TerminalSession and TerminalView decode-to-visible", standaloneDecodeP50, id: "standalone_terminal.decode_visible"),
            metric("standalone TerminalSession and TerminalView decode-to-visible", percentile(standaloneDecodeVisibleSamples, 0.95), "ms", "p95", id: "standalone_terminal.decode_visible"),
            metric("standalone TerminalSession and TerminalView decode-to-visible", percentile(standaloneDecodeVisibleSamples, 0.99), "ms", "p99", id: "standalone_terminal.decode_visible"),
            metric("standalone TerminalView draw-only", standaloneDrawP50, id: "standalone_terminal.draw_only"),
            metric("standalone TerminalView draw-only", percentile(standaloneDrawOnlySamples, 0.95), "ms", "p95", id: "standalone_terminal.draw_only"),
            metric("standalone TerminalView draw-only", percentile(standaloneDrawOnlySamples, 0.99), "ms", "p99", id: "standalone_terminal.draw_only"),
            metric("embedded-to-standalone terminal decode-to-visible", embeddedDecodeP50 / max(standaloneDecodeP50, 0.000_001), "ratio", "p50", id: "terminal_display.embedded_to_standalone_decode_ratio"),
            metric("embedded-to-standalone terminal draw-only", embeddedDrawP50 / max(standaloneDrawP50, 0.000_001), "ratio", "p50", id: "terminal_display.embedded_to_standalone_draw_ratio"),
            metric("client terminal framed envelope", Double(exactFramedBytes), "bytes", "exact", id: "client_terminal.frame_bytes"),
            metric("embedded terminal draw completions", Double(embeddedDrawCompletions), "frames", "exact", id: "client_terminal.raster_completions"),
            metric("standalone terminal draw completions", Double(standaloneDrawCompletions), "frames", "exact", id: "standalone_terminal.raster_completions"),
        ],
        assertions: [
            Assertion(
                id: "terminal_offsets_exact",
                name: "framed embedded terminal payload and offsets remain exact",
                passed: embeddedOffsetsExact && productionPathExact,
                detail: "every active SessionController delivery decoded one framed SRUITerminalData carrying exactly \(payload.count) payload bytes at offset zero; the renderer-owned TerminalSession ended at \(payload.count); framed envelope byte counts: \(observedFramedBytes.sorted())"
            ),
            Assertion(
                id: "standalone_terminal_offsets_exact",
                name: "standalone terminal payload and offsets remain exact",
                passed: standaloneOffsetsExact,
                detail: "every direct production TerminalSession delivery consumed exactly \(payload.count) payload bytes at offset zero and ended at offset \(payload.count)"
            ),
            Assertion(
                id: "terminal_display_draw_completion",
                name: "standalone and embedded terminal visible completions are actual draws",
                passed: embeddedDrawCompletions == iterations
                    && standaloneDrawCompletions == iterations
                    && compositedContentEvidenceExact,
                detail: "\(embeddedDrawCompletions)/\(iterations) embedded and \(standaloneDrawCompletions)/\(iterations) standalone TerminalView completions crossed the configured draw-visible boundary and produced content-distinct bitmaps; full mode additionally required nonblank, nonuniform client-only composited captures; embedded visibility provenance: \(observedEmbeddedVisibilityProvenances.sorted()); standalone visibility provenance: \(observedStandaloneVisibilityProvenances.sorted()); embedded composited fingerprints: \(observedEmbeddedCompositedFingerprints.sorted()); standalone composited fingerprints: \(observedStandaloneCompositedFingerprints.sorted())"
            ),
            Assertion(
                id: "terminal_display_equivalent",
                name: "standalone and embedded terminal displays render the identical ANSI payload",
                passed: displayContentExact && displayRasterExact,
                detail: "both paths consumed payload SHA-256 \(payloadDigest), decoded exact 256-line content SHA-256 \(expectedContentDigest), and produced byte-identical TerminalView rasters; embedded content digests: \(observedEmbeddedContentDigests.sorted()); standalone content digests: \(observedStandaloneContentDigests.sorted()); embedded bitmap digests: \(observedEmbeddedBitmapDigests.sorted()); standalone bitmap digests: \(observedStandaloneBitmapDigests.sorted())"
            ),
            Assertion(
                id: "terminal_fresh_state",
                name: "standalone and production terminal samples begin from fresh parser and view state",
                passed: freshState,
                detail: "embedded samples negotiate \(terminalProfileURI), register namespace \(namespaceID), and mount through a framed transaction; standalone samples instantiate fresh production TerminalSession and TerminalView pairs; both reproduce exact content SHA-256 \(expectedContentDigest)"
            ),
        ],
        notes: [
            "Embedded decode-to-visible starts before a framed 6,912-byte SRUITerminalData enters BenchmarkTransport, then crosses SessionController, the renderer-owned TerminalSession, the renderer-created TerminalView snapshot subscription, and the configured presentation boundary.",
            "The standalone display baseline feeds the identical Data value at offset zero through fresh production TerminalSession and TerminalView instances, using the embedded view's exact grid and bounds. The reported ratios compare p50 display completion on like-for-like content and geometry.",
            "Full mode starts an exact TerminalView-ROI ScreenCaptureKit stream and verifies a nonblank, nonuniform baseline before timing. After the production payload action completes, it accepts only a later complete compositor sample whose same-frame ROI fingerprint changed; the reported latency is action-start mach time to that sample's ScreenCaptureKit display time. Exact WindowServer identity, display, client/ROI geometry, and absence of every intersecting visible nonzero-alpha window are checked before and after acceptance. Callback receipt remains metadata rather than the latency clock. Observed embedded provenance: \(observedEmbeddedVisibilityProvenances.sorted()); standalone provenance: \(observedStandaloneVisibilityProvenances.sorted()). Smoke uses an explicitly named offscreen bitmap draw and is not presentation evidence.",
            "The Rust section separately compares exact standalone-versus-embedded PTY byte transport and exercises reconnect ring-buffer exhaustion; this Swift section compares terminal emulator and display completion."
        ]
    )
}
