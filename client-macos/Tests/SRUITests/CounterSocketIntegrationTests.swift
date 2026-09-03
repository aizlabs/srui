//
// CounterSocketIntegrationTests.swift
// SRUITests
//
// End-to-end integration test against the real Rust counter server over a Unix socket (§20.2, §22, §29).
//

import Testing
import Foundation
import CryptoKit
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit
import Resources

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
            // Soft-skip if Rust binary is not built locally or in CI
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

    @Test("Fresh HELLO against a seeded counter session applies the catch-up snapshot")
    @MainActor
    func helloCatchUpSnapshotOverUnixSocket() async throws {
        let socketPath = "/tmp/srui-counter-hello-\(UUID().uuidString).sock"
        let repoRoot = Self.repositoryRoot()
        let counterBinary = repoRoot
            .appendingPathComponent("examples/counter/target/debug/counter")

        guard FileManager.default.fileExists(atPath: counterBinary.path) else {
            // Soft-skip if Rust binary is not built locally or in CI
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
        // Omit sessionId so the client sends CLIENT HELLO rather than CLIENT RESUME (§15).
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try await Self.waitForRevision(applier, expected: Revision(1), timeoutSeconds: 5)
        try await AsyncTestSupport.eventually(description: "HELLO catch-up enables event dispatch") {
            controller.isEventDispatchEnabled
        }

        let textID = NodeId(2)
        let buttonID = NodeId(4)

        _ = try await controller.sendActivate(nodeId: buttonID)
        try await Self.waitForRevision(applier, expected: Revision(2), timeoutSeconds: 5)

        let textHandle = try #require(renderer.registry.handle(for: textID))
        let textField = try #require(textHandle.view as? NSTextField)
        #expect(textField.stringValue == "Count: 1")

        await controller.stop()
    }

    @Test("Connection fails cleanly at handshake time when server requires an unsupported profile (§4 inv. 13)")
    @MainActor
    func mismatchedRequiredProfileFailsAtHandshake() async throws {
        let socketPath = "/tmp/srui-counter-mismatch-\(UUID().uuidString).sock"
        let repoRoot = Self.repositoryRoot()
        let counterBinary = repoRoot
            .appendingPathComponent("examples/counter/target/debug/counter")

        guard FileManager.default.fileExists(atPath: counterBinary.path) else {
            // Soft-skip if Rust binary is not built locally or in CI
            return
        }

        let server = Process()
        server.executableURL = counterBinary
        server.arguments = ["--socket", socketPath, "--require-profile", "org.srui.unsupported-feature/1"]
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
            clientCapabilities: [Profile.standardWidgetsV1]
        )

        let failurePromise = ManagedAtomic<SessionFailure?>(nil)
        controller.onFailure = { failure in
            failurePromise.store(failure)
        }

        try await controller.start()

        try await AsyncTestSupport.eventually(description: "handshake failure on profile mismatch against live server") {
            controller.isDiverged && (failurePromise.load() != nil || !controller.isHandshakeComplete)
        }
        #expect(!controller.isHandshakeComplete)
        #expect(applier.lastAppliedRevision == .initial)

        await controller.stop()
    }

    @Test("Image fixture delivers a committed resource that paints the Image node (§14)")
    @MainActor
    func imageFixtureCommitsAndPaintsImageView() async throws {
        let socketPath = "/tmp/srui-counter-image-\(UUID().uuidString).sock"
        let repoRoot = Self.repositoryRoot()
        let counterBinary = repoRoot
            .appendingPathComponent("examples/counter/target/debug/counter")

        try #require(
            FileManager.default.fileExists(atPath: counterBinary.path),
            "counter debug binary missing; build examples/counter first"
        )
        try #require(
            Self.counterSupportsImageFixture(counterBinary),
            "counter binary lacks --image-fixture; rebuild examples/counter"
        )

        let server = Process()
        server.executableURL = counterBinary
        server.arguments = ["--socket", socketPath, "--image-fixture"]
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
        let resourceCache = ResourceCache()
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer,
            resourceCache: resourceCache
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try await Self.waitForRevision(applier, expected: Revision(1), timeoutSeconds: 5)

        let pendingHash = try await Self.waitForImagePendingHash(
            in: renderer,
            timeoutSeconds: 10
        )
        try await Self.waitForCommittedResource(
            matching: pendingHash,
            in: resourceCache,
            timeoutSeconds: 10
        )
        try await AsyncTestSupport.eventually(description: "renderer retains committed image") {
            renderer.resolveResourceImage(pendingHash) != nil
        }

        let imageHandle = try #require(
            renderer.registry.allHandles.first {
                $0.nodeType == .image && $0.pendingResourceHash == pendingHash
            }
        )
        let imageView = try #require(imageHandle.view as? NSImageView)
        let size = try #require(imageView.image?.size)
        #expect(size.width > 0)
        #expect(size.height > 0)
        #expect(imageView.image === renderer.resolveResourceImage(pendingHash))

        await controller.stop()
    }

    @Test("Corrupted resource chunk is dropped; session and placeholder survive (§14)")
    @MainActor
    func corruptedResourceChunkKeepsPlaceholderAndSession() async throws {
        let socketPath = "/tmp/srui-counter-image-corrupt-\(UUID().uuidString).sock"
        let repoRoot = Self.repositoryRoot()
        let counterBinary = repoRoot
            .appendingPathComponent("examples/counter/target/debug/counter")

        try #require(
            FileManager.default.fileExists(atPath: counterBinary.path),
            "counter debug binary missing; build examples/counter first"
        )
        try #require(
            Self.counterSupportsImageFixture(counterBinary),
            "counter binary lacks --image-fixture; rebuild examples/counter"
        )

        let server = Process()
        server.executableURL = counterBinary
        server.arguments = ["--socket", socketPath, "--image-fixture"]
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

        let inner = UnixSocketTransport(socketPath: socketPath)
        let transport = ResourceChunkCorruptingTransport(inner: inner)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let resourceCache = ResourceCache()
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer,
            resourceCache: resourceCache
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try await Self.waitForRevision(applier, expected: Revision(1), timeoutSeconds: 5)

        // Give the server time to push metadata + chunks; corruption should prevent commit.
        try await Task.sleep(nanoseconds: 1_500_000_000)
        #expect(controller.isDiverged == false)
        #expect(controller.isHandshakeComplete)

        // No verified image may land in the cache after a flipped chunk byte.
        let imageHandles = renderer.registry.allHandles.filter { $0.nodeType == .image }
        for handle in imageHandles {
            if let hash = handle.pendingResourceHash {
                #expect(await resourceCache.contains(hash) == false)
            }
            let imageView = try #require(handle.view as? NSImageView)
            #expect(imageView.image != nil)
        }

        // Transactions must still flow: activate the increment button if present.
        if let button = renderer.registry.allHandles.first(where: { $0.nodeType == .button }) {
            let before = applier.lastAppliedRevision
            _ = try await controller.sendActivate(nodeId: button.nodeID)
            try await AsyncTestSupport.eventually(description: "post-corruption activate advances revision") {
                applier.lastAppliedRevision > before
            }
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

    /// Detects whether the counter binary advertises `--image-fixture` via `strings`.
    ///
    /// Returns `false` when the flag is absent or when `strings` cannot run — callers should
    /// `#require` the result rather than soft-passing.
    private static func counterSupportsImageFixture(_ binary: URL) -> Bool {
        // The counter binary has no `--help`; scan its strings for the flag name.
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/strings")
        process.arguments = [binary.path]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            let text = String(data: data, encoding: .utf8) ?? ""
            return text.contains("image-fixture")
        } catch {
            // Fail closed: do not claim fixture support when we cannot verify it.
            return false
        }
    }

    private static func waitForImagePendingHash(
        in renderer: AppKitRenderer,
        timeoutSeconds: TimeInterval
    ) async throws -> ResourceHash {
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while Date() < deadline {
            if let hash = renderer.registry.allHandles
                .first(where: { $0.nodeType == .image })?
                .pendingResourceHash {
                return hash
            }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw SocketIntegrationError.resourceTimeout
    }

    private static func waitForCommittedResource(
        matching hash: ResourceHash,
        in cache: ResourceCache,
        timeoutSeconds: TimeInterval
    ) async throws {
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while Date() < deadline {
            if await cache.contains(hash) {
                return
            }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw SocketIntegrationError.resourceTimeout
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

/// Transport wrapper that flips the first byte of every `ResourceChunk` payload (§14 negative path).
private final class ResourceChunkCorruptingTransport: Transport, @unchecked Sendable {
    private let inner: any Transport

    init(inner: any Transport) {
        self.inner = inner
    }

    func send(data: Data) async throws {
        try await inner.send(data: data)
    }

    func close() async {
        await inner.close()
    }

    func acknowledgeReceived(byteCount: Int) async {
        // Inner chunks are acknowledged as they are decoded below.
    }

    func receiveStream() -> AsyncThrowingStream<Data, Error> {
        let inner = self.inner
        return AsyncThrowingStream { continuation in
            let task = Task {
                var decoder = SRUIMessageStreamDecoder()
                do {
                    for try await chunk in inner.receiveStream() {
                        let messages: [SRUIMessage]
                        do {
                            messages = try decoder.appendAndExtract(incoming: chunk)
                        } catch {
                            continuation.finish(throwing: error)
                            return
                        }
                        for message in messages {
                            var outbound = message
                            if case .resourceChunk(var resourceChunk) = outbound.msg,
                               !resourceChunk.data.isEmpty {
                                resourceChunk.data[0] ^= 0xFF
                                outbound.resourceChunk = resourceChunk
                            }
                            do {
                                continuation.yield(try SRUIFraming.encodeFramed(outbound))
                            } catch {
                                continuation.finish(throwing: error)
                                return
                            }
                        }
                        await inner.acknowledgeReceived(byteCount: chunk.count)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in
                task.cancel()
            }
        }
    }
}

private enum SocketIntegrationError: Error, CustomStringConvertible {
    case socketTimeout(String)
    case revisionTimeout(expected: Revision, actual: Revision)
    case resourceTimeout

    var description: String {
        switch self {
        case .socketTimeout(let path):
            return "Timed out waiting for Unix socket at \(path)"
        case .revisionTimeout(let expected, let actual):
            return "Timed out waiting for revision \(expected), still at \(actual)"
        case .resourceTimeout:
            return "Timed out waiting for a committed resource in the client cache"
        }
    }
}
