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

        guard FileManager.default.fileExists(atPath: counterBinary.path) else {
            Issue.record("Counter binary not found at \(counterBinary.path). Run: cargo build --manifest-path examples/counter/Cargo.toml")
            return
        }
        guard FileManager.default.fileExists(atPath: bridgeBinary.path) else {
            Issue.record("srui-ssh-bridge binary not found at \(bridgeBinary.path). Run: cargo build --manifest-path server-rust/Cargo.toml")
            return
        }

        let tempDir = URL(fileURLWithPath: "/tmp/srui-live-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let socketPath = tempDir.appendingPathComponent("c.sock").path
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
        let port: UInt16 = Self.findFreePort()
        let hostPubStr = try String(contentsOf: URL(fileURLWithPath: "\(hostKeyPath).pub"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let knownHostsEntry = "[127.0.0.1]:\(port) \(hostPubStr)\n"
        try knownHostsEntry.write(to: URL(fileURLWithPath: knownHostsPath), atomically: true, encoding: .utf8)

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
        let sshd = Process()
        sshd.executableURL = URL(fileURLWithPath: "/usr/sbin/sshd")
        sshd.arguments = [
            "-f", sshdConfigPath,
            "-h", hostKeyPath,
            "-D", // Do not detach as daemon
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
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try await Self.waitForRevision(applier, expected: Revision(1), timeoutSeconds: 5)

        let textID = NodeId(2)
        let buttonID = NodeId(4)

        // 4. Perform 3 consecutive click-and-observe cycles over SSH transport
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
        throw SSHIntegrationError.timeout("Timed out waiting for socket at \(path)")
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
        throw SSHIntegrationError.timeout("Timed out waiting for revision \(expected), still at \(applier.lastAppliedRevision)")
    }

    private static func findFreePort() -> UInt16 {
        let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return UInt16.random(in: 23000...28000) }
        defer { Darwin.close(fd) }
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        addr.sin_port = 0
        let len = socklen_t(MemoryLayout<sockaddr_in>.size)
        let bindRes = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                Darwin.bind(fd, sa, len)
            }
        }
        if bindRes == 0 {
            var actualAddr = sockaddr_in()
            var actualLen = len
            let getRes = withUnsafeMutablePointer(to: &actualAddr) { ptr in
                ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                    Darwin.getsockname(fd, sa, &actualLen)
                }
            }
            if getRes == 0 {
                return UInt16(bigEndian: actualAddr.sin_port)
            }
        }
        return UInt16.random(in: 23000...28000)
    }
}

private enum SSHIntegrationError: Error, CustomStringConvertible {
    case timeout(String)

    var description: String {
        switch self {
        case .timeout(let msg):
            return msg
        }
    }
}
