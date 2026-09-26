// Live Task 31 composition against the Rust coding-agent demo (§11.1, §20.2, §30).

import Accessibility
import AppKit
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
import Session
import Terminal
import Testing
import Text
import TransportSSH

@Suite("Coding-agent fallback socket integration")
struct CodingAgentFallbackSocketTests {
    @Test("Base client renders fallback, terminal, actions, and TEXT_EDIT over Unix socket")
    @MainActor
    func baseClientRunsCompleteCodingAgentComposition() async throws {
        let socketPath = "/tmp/srui-coding-agent-\(UUID().uuidString).sock"
        let binary = Self.repositoryRoot()
            .appendingPathComponent("examples/coding-agent-demo/target/debug/coding-agent-demo")
        try #require(
            FileManager.default.fileExists(atPath: binary.path),
            "coding-agent demo binary missing; build examples/coding-agent-demo before Swift tests"
        )

        let server = Process()
        server.executableURL = binary
        server.arguments = ["--socket", socketPath]
        server.standardOutput = FileHandle.nullDevice
        server.standardError = FileHandle.nullDevice
        try server.run()
        defer {
            if server.isRunning { server.terminate() }
            server.waitUntilExit()
            try? FileManager.default.removeItem(atPath: socketPath)
        }
        try await Self.waitForSocket(at: socketPath)

        let transport = UnixSocketTransport(socketPath: socketPath)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await Self.waitForRevision(applier, expected: Revision(3))
        try await AsyncTestSupport.eventually(description: "coding-agent snapshot rendered") {
            renderer.registry.handle(for: NodeId(18)) != nil
        }

        let surfaceWindow = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        // Leave nothing in the window list behind this test.
        defer { surfaceWindow.orderOut(nil) }
        #expect(surfaceWindow.styleMask.contains(.resizable))
        #expect(surfaceWindow.contentMaxSize.width > surfaceWindow.contentMinSize.width)
        surfaceWindow.setContentSize(NSSize(width: 900, height: 900))
        surfaceWindow.contentView?.layoutSubtreeIfNeeded()
        for nodeID in [
            NodeId(4), NodeId(5), NodeId(7), NodeId(9), NodeId(12),
            NodeId(13), NodeId(14), NodeId(15), NodeId(17), NodeId(18), NodeId(19),
            NodeId(20), NodeId(21),
        ] {
            let view = try #require(
                renderer.registry.view(for: nodeID),
                "Expected a native view for node \(nodeID)"
            )
            #expect(view.frame.width > 0, "Node \(nodeID) must have visible width")
            #expect(view.frame.height > 0, "Node \(nodeID) must have visible height")
            #expect(view.visibleRect.isEmpty == false, "Node \(nodeID) must not be clipped away")
        }

        let diffProfile = try Profile.parse("org.example.diff/1")
        #expect(controller.negotiatedCapabilities?.contains(diffProfile) == false)
        #expect(renderer.registry.view(for: NodeId(10)) is NSStackView)
        #expect(
            (renderer.registry.view(for: NodeId(12)) as? NSTextField)?.stringValue
                == "Proposed Changes: src/auth.rs"
        )
        let diffScroll = try #require(
            renderer.registry.view(for: NodeId(13)) as? NSScrollView
        )
        let diffView = try #require(diffScroll.documentView as? NSTextView)
        #expect(diffView.string.contains("validate_token"))
        let diffContainer = try #require(diffView.textContainer)
        diffView.layoutManager?.ensureLayout(for: diffContainer)
        let requiredDiffHeight = (diffView.layoutManager?.usedRect(for: diffContainer).height ?? 0)
            + (2 * diffView.textContainerInset.height)
        #expect(requiredDiffHeight <= diffView.bounds.height)
        let terminalView = try #require(
            renderer.registry.view(for: NodeId(14)) as? TerminalView
        )
        #expect(
            applier.store.getNode(NodeId(1))?.getProperty(.horizontalAlignment)?.asEnumToken
                == .horizontalAlignmentFill
        )
        #expect(
            applier.store.getNode(NodeId(6))?.getProperty(.verticalAlignment)?.asEnumToken
                == .verticalAlignmentFill
        )
        #expect(
            applier.store.getNode(NodeId(8))?.getProperty(.horizontalAlignment)?.asEnumToken
                == .horizontalAlignmentFill
        )
        #expect(
            applier.store.getNode(NodeId(14))?.getProperty(.grow)?.asFloat64 == 1
        )
        #expect(
            applier.store.getNode(NodeId(14))?.getProperty(.minimumSize)?.asSize
                == Size(width: 656, height: 480)
        )
        #expect(
            applier.store.getNode(NodeId(7))?.getProperty(.maximumSize)?.asSize?.width == 220
        )
        #expect(renderer.controlFactory.extensionKind(for: TypeRef(namespaceID: 1, localID: 1)) == nil)
        let initialTerminalSize = terminalView.bounds.size
        #expect(initialTerminalSize.width >= 656)
        #expect(initialTerminalSize.height >= 480)
        var reportedTerminalSizes: [(columns: UInt32, rows: UInt32)] = []
        let forwardResize = terminalView.onResize
        terminalView.onResize = { columns, rows, width, height in
            reportedTerminalSizes.append((columns, rows))
            forwardResize?(columns, rows, width, height)
        }
        terminalView.layout()
        let terminalResizeDeadline = Date().addingTimeInterval(3)
        while Date() < terminalResizeDeadline,
              !reportedTerminalSizes.contains(where: {
                  $0.columns >= TerminalView.conventionalColumns
                      && $0.rows >= TerminalView.conventionalRows
              }) {
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        #expect(
            reportedTerminalSizes.contains {
                $0.columns >= TerminalView.conventionalColumns
                    && $0.rows >= TerminalView.conventionalRows
            },
            "Terminal did not publish the minimum PTY size: \(reportedTerminalSizes)"
        )

        let semanticInspector = controller.makeSemanticInspector()
        let semanticApprove = try #require(
            semanticInspector.find(role: .button, label: "Approve")
        )
        _ = try await semanticApprove.activate()
        try await Self.waitForRevision(applier, expected: Revision(4))
        try await AsyncTestSupport.eventually(description: "approval rendered") {
            let conversation = (renderer.registry.view(for: NodeId(9)) as? NSScrollView)?
                .documentView as? NSTextView
            return conversation?.string.contains("approved") == true
                && (renderer.registry.view(for: NodeId(5)) as? NSProgressIndicator)?.doubleValue
                    == 0.75
        }
        let approveButton = try #require(
            renderer.registry.view(for: NodeId(17)) as? NSButton
        )
        let conversationScroll = try #require(
            renderer.registry.view(for: NodeId(9)) as? NSScrollView
        )
        let conversation = try #require(conversationScroll.documentView as? NSTextView)
        #expect(conversation.string.contains("approved"))

        let stableWindowSize = surfaceWindow.frame.size
        let rejectButton = try #require(
            renderer.registry.view(for: NodeId(18)) as? NSButton
        )
        var expectedRevision: UInt64 = 4
        for index in 0..<10 {
            let isReject = index.isMultiple(of: 2)
            (isReject ? rejectButton : approveButton).performClick(nil)
            expectedRevision += 1
            try await Self.waitForRevision(
                applier,
                expected: Revision(expectedRevision)
            )
        }
        surfaceWindow.contentView?.layoutSubtreeIfNeeded()
        #expect(abs(surfaceWindow.frame.width - stableWindowSize.width) < 1)
        #expect(abs(surfaceWindow.frame.height - stableWindowSize.height) < 1)
        #expect(conversationScroll.hasVerticalScroller)
        #expect(conversation.string.components(separatedBy: "User approved").count - 1 == 6)
        #expect(conversation.string.components(separatedBy: "User rejected").count - 1 == 5)
        #expect(
            (renderer.registry.view(for: NodeId(5)) as? NSProgressIndicator)?.doubleValue
                == 0.75
        )

        let promptHandle = try #require(renderer.registry.handle(for: NodeId(15)))
        let promptAdapter = try #require(promptHandle.textAdapter)
        let promptView = try #require(
            (promptHandle.view as? NSScrollView)?.documentView as? NSTextView
        )
        promptView.string = "Add an expiry test"
        promptAdapter.notifyTextDidChangeForTests()
        promptAdapter.notifyEndEditingForTests()
        try await Self.waitForRevision(applier, expected: Revision(15))
        #expect(
            applier.store.getNode(NodeId(15))?.getProperty(.value)?.asString
                == "Add an expiry test"
        )

        await controller.stop()
    }

    private static func repositoryRoot() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private static func waitForSocket(at path: String) async throws {
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            if FileManager.default.fileExists(atPath: path) { return }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        throw CodingAgentSocketError.socketTimeout(path)
    }

    private static func waitForRevision(
        _ applier: TransactionApplier,
        expected: Revision
    ) async throws {
        let deadline = Date().addingTimeInterval(8)
        while Date() < deadline {
            if applier.lastAppliedRevision == expected { return }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw CodingAgentSocketError.revisionTimeout(
            expected: expected,
            actual: applier.lastAppliedRevision
        )
    }
}

private enum CodingAgentSocketError: Error {
    case socketTimeout(String)
    case revisionTimeout(expected: Revision, actual: Revision)
}
