//
// SSHTransportLiveIntegrationTests.swift
// SRUITests
//
// End-to-end integration test executing counter demo cycles over real SSH subsystem transport (§19, §19.1, §20.1, §22, §29).
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

@Suite("SSH Transport Live Integration Tests (§19, §19.1, §20.1)")
struct SSHTransportLiveIntegrationTests {

    @Test("Three activate cycles over real SSH subsystem against live counter server")
    @MainActor
    func threeActivateCyclesOverSSHSubsystem() async throws {
        let repoRoot = Self.repositoryRoot()
        let counterBinary = repoRoot.appendingPathComponent("examples/counter/target/debug/counter")
        let bridgeBinary = repoRoot.appendingPathComponent("server-rust/target/debug/srui-ssh-bridge")

        guard FileManager.default.fileExists(atPath: counterBinary.path) &&
              FileManager.default.fileExists(atPath: bridgeBinary.path) else {
            // Soft-skip if Rust binaries are not built locally or in CI
            return
        }

        let tempDir = try TestFixtureDirectory.make(prefix: "srui-live")
        defer { TestFixtureDirectory.release(tempDir) }

        let socketPath = tempDir.appendingPathComponent("c.sock").path
        let hostKeyPath = tempDir.appendingPathComponent("host_key").path
        let userKeyPath = tempDir.appendingPathComponent("user_key").path
        let authKeysPath = tempDir.appendingPathComponent("authorized_keys").path
        let knownHostsPath = tempDir.appendingPathComponent("known_hosts").path
        let sshdConfigPath = tempDir.appendingPathComponent("sshd_config").path

        try SSHTestSupport.generateEd25519Key(at: hostKeyPath)
        try SSHTestSupport.generateEd25519Key(at: userKeyPath)

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

        // Write sshd_config configuring srui subsystem to invoke srui-ssh-bridge pointing to counter socket (§19, §20.1)
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

        // 1. Launch Rust counter server
        let counterServer = Process()
        counterServer.executableURL = counterBinary
        counterServer.arguments = ["--socket", socketPath]
        counterServer.standardOutput = FileHandle.nullDevice
        counterServer.standardError = FileHandle.nullDevice
        try counterServer.run()
        defer {
            if counterServer.isRunning {
                counterServer.terminate()
            }
            counterServer.waitUntilExit()
        }

        try await Self.waitForSocket(at: socketPath, timeoutSeconds: 5)

        // 2. Launch ephemeral sshd daemon
        let sshd = try SSHTestSupport.launchSSHD(
            configPath: sshdConfigPath,
            hostKeyPath: hostKeyPath,
            port: port)
        defer { SSHTestSupport.terminate(sshd) }

        try await SSHTestSupport.waitForPort(port: port, timeoutSeconds: 5)

        // 3. Connect Swift client via SSHTransport conforming strictly to §19.1
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
        let transport = SSHTransport(configuration: sshConfig)

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
        // Every wait in this test crosses a real SSH connection: an `ssh` client process, sshd's
        // authentication, the bridge subsystem exec and the sessiond handshake. The 2s default is
        // a local-actor budget; on a 3-core runner sharing itself with the whole suite that round
        // trip is seconds, so the deadline has to cover the transport rather than a hop.
        try await AsyncTestSupport.eventually(
            timeout: .seconds(15),
            description: "initial revision over SSH"
        ) {
            applier.lastAppliedRevision == Revision(1)
        }

        let textID = NodeId(2)
        let buttonID = NodeId(4)

        // 4. Perform 3 consecutive click-and-observe cycles over SSH transport
        for cycle in 1...3 {
            _ = try await controller.sendActivate(nodeId: buttonID)
            try await AsyncTestSupport.eventually(
                timeout: .seconds(15),
                description: "counter cycle \(cycle) over SSH"
            ) {
                guard applier.lastAppliedRevision == Revision(UInt64(cycle + 1)) else {
                    return false
                }
                guard let textHandle = renderer.registry.handle(for: textID),
                      let textField = textHandle.view as? NSTextField else {
                    return false
                }
                return textField.stringValue == "Count: \(cycle)"
            }

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
        throw AsyncTestTimeout(description: "Timed out waiting for socket at \(path)")
    }
}
