//
// SSHTransportPersistenceIntegrationTests.swift
// SRUITests
//
// End-to-end integration test verifying sessiond persistence across SSH disconnects
// and globally unique session_id incarnation token semantics (§17, §19, §19.1, §20.2, §22, §29).
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

@Suite("SSH Transport Persistence Integration Tests (§17, §20.2)")
struct SSHTransportPersistenceIntegrationTests {

    @Test("Sessiond survives SSH bridge death; fresh Connection B preserves session_id and committed state")
    @MainActor
    func sessiondSurvivesBridgeDeathAndPreservesState() async throws {
        let repoRoot = Self.repositoryRoot()
        let sessiondBinary = repoRoot.appendingPathComponent("server-rust/target/debug/srui-sessiond")
        let bridgeBinary = repoRoot.appendingPathComponent("server-rust/target/debug/srui-ssh-bridge")

        guard FileManager.default.fileExists(atPath: sessiondBinary.path) else {
            Issue.record("srui-sessiond binary not found at \(sessiondBinary.path). Run: cargo build --manifest-path server-rust/Cargo.toml")
            return
        }
        guard FileManager.default.fileExists(atPath: bridgeBinary.path) else {
            Issue.record("srui-ssh-bridge binary not found at \(bridgeBinary.path). Run: cargo build --manifest-path server-rust/Cargo.toml")
            return
        }

        let tempDir = URL(fileURLWithPath: "/tmp/srui-persist-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let socketPath = tempDir.appendingPathComponent("sessiond.sock").path
        let hostKeyPath = tempDir.appendingPathComponent("host_key").path
        let userKeyPath = tempDir.appendingPathComponent("user_key").path
        let authKeysPath = tempDir.appendingPathComponent("authorized_keys").path
        let knownHostsPath = tempDir.appendingPathComponent("known_hosts").path
        let sshdConfigPath = tempDir.appendingPathComponent("sshd_config").path

        // Generate SSH host key
        let genHostKey = Process()
        genHostKey.executableURL = URL(fileURLWithPath: "/usr/bin/ssh-keygen")
        genHostKey.arguments = ["-t", "ed25519", "-N", "", "-f", hostKeyPath]
        try genHostKey.run()
        genHostKey.waitUntilExit()

        // Generate SSH user key
        let genUserKey = Process()
        genUserKey.executableURL = URL(fileURLWithPath: "/usr/bin/ssh-keygen")
        genUserKey.arguments = ["-t", "ed25519", "-N", "", "-f", userKeyPath]
        try genUserKey.run()
        genUserKey.waitUntilExit()

        // Install user public key in authorized_keys
        let userPubData = try Data(contentsOf: URL(fileURLWithPath: "\(userKeyPath).pub"))
        try userPubData.write(to: URL(fileURLWithPath: authKeysPath))

        // Write host public key to known_hosts
        let port = SSHTestSupport.findFreePort()
        try SSHTestSupport.writeKnownHosts(
            port: port,
            hostPublicKeyPath: hostKeyPath,
            to: knownHostsPath
        )

        // Write sshd_config configuring srui subsystem to invoke srui-ssh-bridge pointing to sessiond socket
        let sshdConfigContent = """
        Port \(port)
        HostKey \(hostKeyPath)
        AuthorizedKeysFile \(authKeysPath)
        StrictModes no
        UsePAM no
        PidFile \(tempDir.appendingPathComponent("sshd.pid").path)
        Subsystem srui \(bridgeBinary.path) \(socketPath)
        """
        try sshdConfigContent.write(to: URL(fileURLWithPath: sshdConfigPath), atomically: true, encoding: .utf8)

        // 1. Launch persistent srui-sessiond daemon hosting the counter application (§20.2)
        let sessiond = Process()
        sessiond.executableURL = sessiondBinary
        sessiond.arguments = ["--socket", socketPath, "--app", "counter"]
        sessiond.standardOutput = FileHandle.nullDevice
        sessiond.standardError = FileHandle.nullDevice
        try sessiond.run()
        defer {
            if sessiond.isRunning {
                sessiond.terminate()
            }
            sessiond.waitUntilExit()
        }

        try await Self.waitForSocket(at: socketPath, timeoutSeconds: 5)

        // 2. Launch ephemeral sshd daemon
        let sshd = Process()
        sshd.executableURL = URL(fileURLWithPath: "/usr/sbin/sshd")
        sshd.arguments = [
            "-f", sshdConfigPath,
            "-h", hostKeyPath,
            "-D",
            "-p", String(port)
        ]
        try sshd.run()
        defer {
            if sshd.isRunning {
                sshd.terminate()
            }
            sshd.waitUntilExit()
        }

        try await Task.sleep(nanoseconds: 300_000_000)

        let sshConfig = SSHConfiguration(
            host: "127.0.0.1",
            port: port,
            subsystem: "srui",
            identityFile: userKeyPath,
            knownHostsFile: knownHostsPath,
            strictHostKeyChecking: .yes,
            batchMode: true,
            connectTimeout: 5.0
        )

        // 3. Attach Connection A over SSH
        let transportA = SSHTransport(configuration: sshConfig)
        let applierA = TransactionApplier()
        let rendererA = AppKitRenderer()
        let controllerA = SessionController(
            transport: transportA,
            applier: applierA,
            renderer: rendererA
        )
        controllerA.attachRenderer(rendererA)

        try await controllerA.start()
        try await AsyncTestSupport.eventually(description: "connection A initial revision over SSH") {
            applierA.lastAppliedRevision == Revision(1)
        }

        let buttonID = NodeId(4)
        let textID = NodeId(2)

        // Run 3 click cycles on Connection A
        for cycle in 1...3 {
            _ = try await controllerA.sendActivate(nodeId: buttonID)
            try await AsyncTestSupport.eventually(description: "connection A cycle \(cycle)") {
                guard applierA.lastAppliedRevision == Revision(UInt64(cycle + 1)) else {
                    return false
                }
                guard let textHandle = rendererA.registry.handle(for: textID),
                      let textField = textHandle.view as? NSTextField else {
                    return false
                }
                return textField.stringValue == "Count: \(cycle)"
            }
        }

        let sessionIDA = try #require(controllerA.sessionId, "Connection A must have received session_id")
        #expect(!sessionIDA.isEmpty)

        // 4. Forcibly terminate Connection A (transport drop / bridge death)
        await controllerA.stop()
        try await Task.sleep(nanoseconds: 200_000_000)

        // Confirm sessiond is still running and persistent!
        #expect(sessiond.isRunning, "sessiond daemon must survive SSH bridge / connection death")

        // 5. Attach fresh Connection B over SSH to the persistent sessiond
        let transportB = SSHTransport(configuration: sshConfig)
        let applierB = TransactionApplier()
        let rendererB = AppKitRenderer()
        let controllerB = SessionController(
            transport: transportB,
            applier: applierB,
            renderer: rendererB
        )
        controllerB.attachRenderer(rendererB)

        try await controllerB.start()
        try await AsyncTestSupport.eventually(description: "connection B receives preserved state over SSH") {
            guard applierB.lastAppliedRevision == Revision(4) else {
                return false
            }
            guard let textHandle = rendererB.registry.handle(for: textID),
                  let textField = textHandle.view as? NSTextField else {
                return false
            }
            return textField.stringValue == "Count: 3"
        }

        let sessionIDB = try #require(controllerB.sessionId, "Connection B must have received session_id")
        #expect(sessionIDB == sessionIDA, "session_id incarnation token must be identical across connections to same daemon")

        let textHandleB = try #require(rendererB.registry.handle(for: textID))
        let textFieldB = try #require(textHandleB.view as? NSTextField)
        #expect(textFieldB.stringValue == "Count: 3", "committed semantic state Count: 3 must survive transport disconnection")

        // 6. Perform click on Connection B and verify counter increments to Count: 4
        _ = try await controllerB.sendActivate(nodeId: buttonID)
        try await AsyncTestSupport.eventually(description: "connection B cycle 4") {
            guard applierB.lastAppliedRevision == Revision(5) else {
                return false
            }
            guard let textHandle = rendererB.registry.handle(for: textID),
                  let textField = textHandle.view as? NSTextField else {
                return false
            }
            return textField.stringValue == "Count: 4"
        }

        #expect(textFieldB.stringValue == "Count: 4")
        await controllerB.stop()
    }

    @Test("Restarting sessiond process mints globally unique session_id tokens with no collisions")
    @MainActor
    func sessiondRestartMintsUniqueSessionIds() async throws {
        let repoRoot = Self.repositoryRoot()
        let sessiondBinary = repoRoot.appendingPathComponent("server-rust/target/debug/srui-sessiond")

        guard FileManager.default.fileExists(atPath: sessiondBinary.path) else {
            Issue.record("srui-sessiond binary not found at \(sessiondBinary.path)")
            return
        }

        let tempDir = URL(fileURLWithPath: "/tmp/srui-restarts-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        var seenSessionIds = Set<String>()
        constRestartLoop: for iteration in 1...5 {
            let socketPath = tempDir.appendingPathComponent("sessiond-\(iteration).sock").path

            let sessiond = Process()
            sessiond.executableURL = sessiondBinary
            sessiond.arguments = ["--socket", socketPath, "--app", "counter"]
            sessiond.standardOutput = FileHandle.nullDevice
            sessiond.standardError = FileHandle.nullDevice
            try sessiond.run()

            try await Self.waitForSocket(at: socketPath, timeoutSeconds: 5)

            // Connect directly via Unix domain socket transport
            let transport = UnixSocketTransport(socketPath: socketPath)
            let applier = TransactionApplier()
            let controller = SessionController(
                transport: transport,
                applier: applier
            )

            try await controller.start()
            try await AsyncTestSupport.eventually(description: "handshake on restart \(iteration)") {
                controller.sessionId != nil
            }

            let sid = try #require(controller.sessionId)
            #expect(!sid.isEmpty)
            #expect(!seenSessionIds.contains(sid), "session_id \(sid) collided with a previous process run!")
            seenSessionIds.insert(sid)

            await controller.stop()
            sessiond.terminate()
            sessiond.waitUntilExit()
        }

        #expect(seenSessionIds.count == 5)
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
