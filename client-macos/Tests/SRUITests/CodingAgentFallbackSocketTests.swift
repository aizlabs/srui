// Live Task 31 composition against the Rust coding-agent demo (§11.1, §20.2, §30).

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

        let diffProfile = try Profile.parse("org.example.diff/1")
        #expect(controller.negotiatedCapabilities?.contains(diffProfile) == false)
        #expect(renderer.registry.view(for: NodeId(10)) is NSStackView)
        #expect(
            (renderer.registry.view(for: NodeId(12)) as? NSTextField)?.stringValue
                == "Proposed Changes: src/auth.rs"
        )
        #expect(
            (renderer.registry.view(for: NodeId(13)) as? NSTextView)?.string
                .contains("validate_token") == true
        )
        #expect(renderer.registry.view(for: NodeId(14)) is TerminalView)
        #expect(renderer.controlFactory.extensionKind(for: TypeRef(namespaceID: 1, localID: 1)) == nil)

        _ = try await controller.sendActivate(nodeId: NodeId(17))
        try await Self.waitForRevision(applier, expected: Revision(4))
        try await AsyncTestSupport.eventually(description: "approval rendered") {
            (renderer.registry.view(for: NodeId(9)) as? NSTextView)?.string
                .contains("approved") == true
        }

        let promptHandle = try #require(renderer.registry.handle(for: NodeId(15)))
        let promptAdapter = try #require(promptHandle.textAdapter)
        let promptView = try #require(
            (promptHandle.view as? NSScrollView)?.documentView as? NSTextView
        )
        promptView.string = "Add an expiry test"
        promptAdapter.notifyTextDidChangeForTests()
        promptAdapter.notifyEndEditingForTests()
        try await Self.waitForRevision(applier, expected: Revision(5))
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
