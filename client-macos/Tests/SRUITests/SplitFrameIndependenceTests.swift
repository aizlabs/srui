//
// SplitFrameIndependenceTests.swift
// SRUITests
//
// PX-070-G02, §32 suite 4 (frame independence) for the standard Split container (§4.16, §7.6,
// §12.2, §22.5). The real generic client (SessionController + AppKitRenderer) is connected to an
// in-process server over a PipeTransport and mounts a Surface holding a two-pane Split and a
// Button. Each run captures every frame the client sends after the mount. One run leaves the
// divider alone; the other moves it repeatedly through NSSplitView and through its accessibility
// splitter. Both runs then press the Button once.
//
// Asserted: the divider moves add no frame at all, and the two runs' streams are byte-identical
// once each event's random `event_id` and `client_instance_id` are blanked (fresh UUIDs per
// event and per client run by design). The
// Button press is the positive control that the capture is live.
//

import AppKit
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import TransportSSH

/// Every message the client sent, in order, re-encoded as framed bytes.
private actor FrameCapture {
    private(set) var frames: [Data] = []
    private var task: Task<Void, Never>?

    func start(draining transport: any Transport) {
        let stream = transport.receiveStream()
        task = Task { [weak self] in
            var decoder = SRUIMessageStreamDecoder()
            do {
                for try await chunk in stream {
                    for message in try decoder.appendAndExtract(incoming: chunk) {
                        await self?.append(try SRUIFraming.encodeFramed(message))
                    }
                }
            } catch {
                // Teardown closes the transport.
            }
        }
    }

    private func append(_ frame: Data) { frames.append(frame) }
    func stop() { task?.cancel() }
}

@Suite(.timeLimit(.minutes(1)))
@MainActor
struct SplitFrameIndependenceTests {
    private static let surfaceID = NodeId(1)
    private static let splitID = NodeId(2)
    private static let buttonID = NodeId(5)

    /// Frames sent after the mount; `resize` moves the divider before the Button press.
    private func run(resize: Bool) async throws -> (afterMount: Int, duringResize: Int, frames: [Data]) {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(transport: clientTransport, applier: applier, renderer: renderer)
        controller.attachRenderer(renderer)
        let capture = FrameCapture()
        await capture.start(draining: serverTransport)
        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "split-frame-independence"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))
        let mount = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: Self.surfaceID, nodeType: .surface),
                .createNode(id: Self.splitID, nodeType: .split, parentID: Self.surfaceID,
                            properties: [(.splitAxis, .enumToken(.splitAxisStacked))]),
                .createNode(id: 3, nodeType: .text, parentID: Self.splitID, properties: [(.text, .string("panels"))]),
                .createNode(id: 4, nodeType: .text, parentID: Self.splitID, properties: [(.text, .string("table"))]),
                .createNode(id: Self.buttonID, nodeType: .button, parentID: Self.surfaceID,
                            properties: [(.label, .string("Refresh"))]),
            ]
        )
        var mountMessage = SRUIMessage()
        mountMessage.transaction = mount.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(mountMessage))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "split mounted") {
            applier.lastAppliedRevision == Revision(1) && renderer.registry.handle(for: Self.splitID) != nil
        }
        #expect(controller.isEventDispatchEnabled)

        let window = try #require(renderer.registry.handle(for: Self.surfaceID)?.window)
        window.setContentSize(NSSize(width: 700, height: 600))
        window.contentView?.layoutSubtreeIfNeeded()
        // Let everything the mount causes reach the capture before counting.
        try await Task.sleep(for: .milliseconds(300))
        let afterMount = await capture.frames.count

        if resize {
            let split = try #require(renderer.registry.view(for: Self.splitID) as? SemanticSplitView)
            for position in [120.0, 400.0, 260.0] {
                split.setPosition(CGFloat(position), ofDividerAt: 0)
                window.contentView?.layoutSubtreeIfNeeded()
            }
            let splitter = try #require(
                (split.accessibilityChildren() ?? []).compactMap { $0 as? SplitDividerAccessibilityElement }.first
            )
            #expect(splitter.accessibilityPerformIncrement())
            #expect(splitter.accessibilityPerformDecrement())
            #expect(abs((split.positionOfDivider(at: 0) ?? 0) - 260) < 1, "the divider really moved")
        }
        try await Task.sleep(for: .milliseconds(300))
        let duringResize = await capture.frames.count

        // Positive control: one Button press is one frame on the wire.
        let button = try #require(renderer.registry.handle(for: Self.buttonID))
        let trampoline = try #require(button.actionTrampoline as? ActionTrampoline)
        trampoline.performButtonAction(button.view)
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "ACTIVATE frame") {
            await capture.frames.count > duringResize
        }
        try await Task.sleep(for: .milliseconds(200))
        let frames = Array(await capture.frames.dropFirst(afterMount))

        await controller.stop()
        await capture.stop()
        await clientTransport.close()
        await serverTransport.close()
        window.close()
        return (afterMount, duringResize, frames)
    }

    /// Blanks each event's random `event_id` and `client_instance_id` (fresh UUIDs per event and
    /// per client run by design); everything else is compared byte for byte.
    private func normalized(_ frames: [Data]) throws -> [Data] {
        try frames.map { frame in
            var message = try SRUIFraming.decodeFramed(SRUIMessage.self, from: frame)
            if case .event(var event) = message.msg {
                #expect(!event.eventID.isEmpty)
                event.eventID = Data()
                // A fresh client instance per run (a random UUID, like event_id).
                #expect(!event.clientInstanceID.isEmpty)
                event.clientInstanceID = Data()
                message.event = event
            }
            return try SRUIFraming.encodeFramed(message)
        }
    }

    @Test
    func movingTheDividerChangesNoProtocolTraffic() async throws {
        let still = try await run(resize: false)
        let moved = try await run(resize: true)
        #expect(still.duringResize == still.afterMount)
        #expect(moved.duringResize == moved.afterMount, "divider moves sent \(moved.duringResize - moved.afterMount) frame(s)")
        #expect(still.frames.count == 1 && moved.frames.count == 1, "exactly the Button's ACTIVATE after the mount")
        let movedFrames = try normalized(moved.frames), stillFrames = try normalized(still.frames)
        #expect(movedFrames == stillFrames, "the streams with and without local resizing are byte-identical: \(movedFrames.map { $0.base64EncodedString() }) vs \(stillFrames.map { $0.base64EncodedString() })")
    }
}
