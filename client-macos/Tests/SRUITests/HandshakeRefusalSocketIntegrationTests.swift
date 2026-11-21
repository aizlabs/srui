//
// HandshakeRefusalSocketIntegrationTests.swift
// SRUITests
//
// PX-026-G02: the real `srui-sessiond` refuses an incompatible client with
// `SERVER HANDSHAKE_REFUSED{UNSPECIFIED, detail}` before closing, over a real Unix socket, and the
// native client reports that detail instead of a closed stream (§15, §18, §19.2, §26).
//

import Testing
import Foundation
import SemanticModel
import Protocol
import Session
import TransportSSH

@Suite("Handshake refusal against a live sessiond (PX-026-G02)", .timeLimit(.minutes(2)))
struct HandshakeRefusalSocketIntegrationTests {

    @Test("A client lacking a required profile sees the server's refusal, not a closed stream")
    @MainActor
    func missingRequiredProfileIsRefusedClearly() async throws {
        try await Self.withLiveSessiond(prefix: "srui-px026g02-profile") { socketPath in
            let controller = SessionController(
                transport: UnixSocketTransport(socketPath: socketPath),
                clientCapabilities: []
            )
            let failures = ManagedAtomic<[SessionFailure]>([])
            controller.onFailure = { failure in
                failures.store(failures.load() + [failure])
            }
            try await controller.start()
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "refusal reported") {
                !failures.load().isEmpty
            }
            let shown = failures.load().first.map(String.init(describing:)) ?? ""
            #expect(shown == "server refused the handshake: client offer does not satisfy required profiles: [org.srui.standard-widgets/1]")
            #expect(!shown.contains("transport ended"))
            #expect(!controller.isHandshakeComplete)
            await controller.stop()
        }
    }

    @Test("An incompatible core version receives exactly one refusal, then the stream closes")
    @MainActor
    func incompatibleCoreVersionIsRefusedOnTheWire() async throws {
        try await Self.withLiveSessiond(prefix: "srui-px026g02-version") { socketPath in
            let transport = UnixSocketTransport(socketPath: socketPath)
            try await transport.connect()
            var hello = SRUIClientHello()
            hello.coreVersion = "1.5.0"
            hello.profiles = ["org.srui.standard-widgets/1"]
            hello.clientInstanceID = Data("px026g02-live".utf8)
            var message = SRUIMessage()
            message.clientHello = hello
            try await transport.send(data: try SRUIFraming.encodeFramed(message))

            var decoder = SRUIMessageStreamDecoder()
            var received: [SRUIMessage.OneOf_Msg] = []
            // The daemon closes after the refusal, so the stream ends on its own.
            for try await chunk in transport.receiveStream() {
                for decoded in try decoder.appendAndExtract(incoming: chunk) {
                    if let msg = decoded.msg { received.append(msg) }
                }
            }
            await transport.close()

            #expect(received.count == 1, "exactly one frame precedes the close: \(received)")
            guard case .serverHandshakeRefused(let refusal)? = received.first else {
                Issue.record("expected SERVER HANDSHAKE_REFUSED, got \(received)")
                return
            }
            #expect(refusal.reason == .unspecified)
            #expect(refusal.detail == "unsupported core protocol version 1.5.0 (this server serves 0.5)")
        }
    }

    /// Runs `body` against a freshly launched `srui-sessiond --app counter`, then stops the daemon
    /// and awaits its exit through the termination handler (never `waitUntilExit()`, which would
    /// service the main run loop of this main-actor test).
    @MainActor
    private static func withLiveSessiond(
        prefix: String,
        _ body: @MainActor (String) async throws -> Void
    ) async throws {
        let sessiondBinary = repositoryRoot().appendingPathComponent("server-rust/target/debug/srui-sessiond")
        guard FileManager.default.fileExists(atPath: sessiondBinary.path) else {
            // Soft-skip if the Rust binary is not built, as every live sessiond suite does.
            return
        }
        let tempDir = try TestFixtureDirectory.make(prefix: prefix)
        defer { TestFixtureDirectory.release(tempDir) }
        let socketPath = tempDir.appendingPathComponent("sessiond.sock").path

        try await FixtureBinaries.absorbFirstLaunchAssessment(
            sessiondBinary.path, arguments: FixtureBinaries.sessiondRefusal)
        let sessiond = Process()
        sessiond.executableURL = sessiondBinary
        sessiond.arguments = ["--socket", socketPath, "--app", "counter"]
        sessiond.standardInput = FileHandle.nullDevice
        sessiond.standardOutput = FileHandle.nullDevice
        sessiond.standardError = FileHandle.nullDevice
        let (exited, exitedContinuation) = AsyncStream<Void>.makeStream()
        sessiond.terminationHandler = { _ in
            exitedContinuation.yield()
            exitedContinuation.finish()
        }
        try sessiond.run()

        let outcome: Result<Void, Error>
        do {
            try await waitForSocket(at: socketPath, timeoutSeconds: 10)
            try await body(socketPath)
            outcome = .success(())
        } catch {
            outcome = .failure(error)
        }
        if sessiond.isRunning {
            sessiond.terminate()
        }
        for await _ in exited {}
        try outcome.get()
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
        throw AsyncTestTimeout(description: "Timed out waiting for socket at \(path)")
    }
}
