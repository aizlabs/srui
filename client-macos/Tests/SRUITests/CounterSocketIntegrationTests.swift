//
// CounterSocketIntegrationTests.swift
// SRUITests
//
// End-to-end integration test against the real Rust counter server over a Unix socket (§20.2, §22, §29).
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

@Suite("Counter Socket Integration Tests")
struct CounterSocketIntegrationTests {

    @Test("Three activate cycles over Unix socket against live counter server")
    @MainActor
    func threeActivateCyclesOverUnixSocket() async throws {
        let socketPath = "/tmp/srui-counter-test-\(UUID().uuidString).sock"
        let repoRoot = Self.repositoryRoot()
        let counterBinary = repoRoot
            .appendingPathComponent("examples/counter/target/debug/counter")

        guard FileManager.default.fileExists(atPath: counterBinary.path) else {
            Issue.record("Counter binary not found at \(counterBinary.path). Run: cargo build --manifest-path examples/counter/Cargo.toml")
            return
        }

        let server = Process()
        server.executableURL = counterBinary
        server.arguments = ["--socket", socketPath]
        server.standardOutput = FileHandle.nullDevice
        server.standardError = FileHandle.nullDevice

        try server.run()
        defer {
            if server.isRunning {
                server.terminate()
            }
            server.waitUntilExit()
            try? FileManager.default.removeItem(atPath: socketPath)
        }

        try await Self.waitForSocket(at: socketPath, timeoutSeconds: 10)

        let transport = UnixSocketTransport(socketPath: socketPath)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer,
            sessionId: "counter-socket-session"
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try await Self.waitForRevision(applier, expected: Revision(1), timeoutSeconds: 5)

        let textID = NodeId(2)
        let buttonID = NodeId(4)

        for cycle in 1...3 {
            _ = try await controller.sendActivate(nodeId: buttonID)
            try await Self.waitForRevision(applier, expected: Revision(UInt64(cycle + 1)), timeoutSeconds: 5)

            let textHandle = try #require(renderer.registry.handle(for: textID))
            let textField = try #require(textHandle.view as? NSTextField)
            #expect(textField.stringValue == "Count: \(cycle)")
        }

        await controller.stop()
    }

    private static func repositoryRoot() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private static func waitForSocket(at path: String, timeoutSeconds: TimeInterval) async throws {
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while Date() < deadline {
            if FileManager.default.fileExists(atPath: path) {
                return
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        throw SocketIntegrationError.socketTimeout(path)
    }

    private static func waitForRevision(
        _ applier: TransactionApplier,
        expected: Revision,
        timeoutSeconds: TimeInterval
    ) async throws {
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while Date() < deadline {
            if applier.lastAppliedRevision == expected {
                return
            }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw SocketIntegrationError.revisionTimeout(expected: expected, actual: applier.lastAppliedRevision)
    }
}

private enum SocketIntegrationError: Error, CustomStringConvertible {
    case socketTimeout(String)
    case revisionTimeout(expected: Revision, actual: Revision)

    var description: String {
        switch self {
        case .socketTimeout(let path):
            return "Timed out waiting for Unix socket at \(path)"
        case .revisionTimeout(let expected, let actual):
            return "Timed out waiting for revision \(expected), still at \(actual)"
        }
    }
}
