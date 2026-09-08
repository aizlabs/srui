//
// SSHTransportHostKeyTests.swift
// SRUITests
//
// Automated test verifying that SSHTransport fails closed on host-key mismatch (§19.1, §25).
//

import Testing
import Foundation
import TransportSSH

@Suite("SSH Host Key Verification Tests (§19.1, §25)")
struct SSHTransportHostKeyTests {

    @Test("SSHTransport fails closed on host key verification mismatch")
    func badHostKeyFailsClosed() async throws {
        let tempDir = URL(fileURLWithPath: "/tmp/srui-hk-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let realHostKeyPath = tempDir.appendingPathComponent("real_host_key").path
        let spoofedHostKeyPath = tempDir.appendingPathComponent("spoofed_host_key").path
        let userKeyPath = tempDir.appendingPathComponent("user_key").path
        let authKeysPath = tempDir.appendingPathComponent("authorized_keys").path
        let knownHostsPath = tempDir.appendingPathComponent("known_hosts").path
        let sshdConfigPath = tempDir.appendingPathComponent("sshd_config").path

        try SSHTestSupport.generateEd25519Key(at: realHostKeyPath)
        try SSHTestSupport.generateEd25519Key(at: spoofedHostKeyPath)
        try SSHTestSupport.generateEd25519Key(at: userKeyPath)

        let userPubData = try Data(contentsOf: URL(fileURLWithPath: "\(userKeyPath).pub"))
        try userPubData.write(to: URL(fileURLWithPath: authKeysPath))

        let port = SSHTestSupport.findFreePort()
        try SSHTestSupport.writeKnownHosts(
            port: port,
            hostPublicKeyPath: spoofedHostKeyPath,
            to: knownHostsPath
        )

        let configContent = """
        Port \(port)
        HostKey \(realHostKeyPath)
        AuthorizedKeysFile \(authKeysPath)
        StrictModes no
        UsePAM no
        PidFile \(tempDir.appendingPathComponent("sshd.pid").path)
        Subsystem srui /bin/echo "srui-subsystem"
        """
        try configContent.write(to: URL(fileURLWithPath: sshdConfigPath), atomically: true, encoding: .utf8)

        let sshdProc = try SSHTestSupport.launchSSHD(
            configPath: sshdConfigPath,
            hostKeyPath: realHostKeyPath,
            port: port,
            debug: true)
        defer { SSHTestSupport.terminate(sshdProc) }

        try await Task.sleep(nanoseconds: 200_000_000)

        let sshConfig = SSHConfiguration(
            host: "127.0.0.1",
            port: port,
            subsystem: "srui",
            identityFile: userKeyPath,
            knownHostsFile: knownHostsPath,
            strictHostKeyChecking: .yes,
            batchMode: true,
            connectTimeout: 3.0
        )
        let transport = SSHTransport(configuration: sshConfig)

        let stream = transport.receiveStream()
        var sawHostKeyFailure = false

        do {
            try await transport.send(data: Data("test".utf8))
            for try await _ in stream {
                Issue.record("Should not receive stream data on host key verification failure")
            }
        } catch let error as TransportError {
            switch error {
            case .connectionFailed(let message):
                sawHostKeyFailure = message.contains("Host key verification failed")
                    || message.contains("exit code")
                #expect(sawHostKeyFailure, "Expected host-key failure message, got: \(message)")
            default:
                Issue.record("Expected TransportError.connectionFailed, got \(error)")
            }
        } catch {
            Issue.record("Expected TransportError, got \(error)")
        }

        #expect(sawHostKeyFailure, "SSHTransport must fail closed when host key verification fails (§19.1, §25)")
        await transport.close()
    }
}
