// PX-001: real non-PTY SSH launch through the unchanged generic client (§§8, 12, 22, 29).
import Testing
import Foundation
import AppKit
import Darwin
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

struct ProcessExplorerShellTests {
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func emptyShellOverSSHRetainsNativeHandlesAfterTitleFixture() async throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let appBinary = root.appendingPathComponent("apps/srtop/target/debug/srtop")
        let bridgeBinary = root.appendingPathComponent("server-rust/target/debug/srui-ssh-bridge")
        try #require(FileManager.default.isExecutableFile(atPath: appBinary.path),
                     "Run bash apps/srtop/test.sh to build the required server")
        try #require(FileManager.default.isExecutableFile(atPath: bridgeBinary.path),
                     "The real SSH bridge is required; this test must not soft-skip")
        let temp = URL(fileURLWithPath: "/tmp/px001-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: temp, withIntermediateDirectories: true,
                                               attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: temp) }
        let socket = temp.appendingPathComponent("s.sock").path
        let hostKey = temp.appendingPathComponent("host_key").path
        let userKey = temp.appendingPathComponent("user_key").path
        let authorizedKeys = temp.appendingPathComponent("authorized_keys")
        let knownHosts = temp.appendingPathComponent("known_hosts").path
        let config = temp.appendingPathComponent("sshd_config")
        try SSHTestSupport.generateEd25519Key(at: hostKey)
        try SSHTestSupport.generateEd25519Key(at: userKey)
        try Data(contentsOf: URL(fileURLWithPath: "\(userKey).pub")).write(to: authorizedKeys)
        let port = SSHTestSupport.findFreePort()
        try SSHTestSupport.writeKnownHosts(port: port, hostPublicKeyPath: hostKey, to: knownHosts)
        try """
        Port \(port)
        ListenAddress 127.0.0.1
        HostKey \(hostKey)
        AuthorizedKeysFile \(authorizedKeys.path)
        StrictModes no
        UsePAM no
        PidFile \(temp.appendingPathComponent("sshd.pid").path)
        Subsystem srui \(bridgeBinary.path) \(socket)
        """.write(to: config, atomically: true, encoding: .utf8)

        let server = Process()
        server.executableURL = appBinary
        server.arguments = ["--socket", socket, "--smoke-fixture"]
        server.standardOutput = FileHandle.nullDevice
        server.standardError = FileHandle.nullDevice
        try server.run()
        defer {
            if server.isRunning { kill(server.processIdentifier, SIGINT) }
            server.waitUntilExit()
        }
        try await AsyncTestSupport.eventually(timeout: .seconds(5), description: "srtop private socket") {
            FileManager.default.fileExists(atPath: socket)
        }
        let sshd = try SSHTestSupport.launchSSHD(configPath: config.path, hostKeyPath: hostKey, port: port)
        defer { SSHTestSupport.terminate(sshd) }
        try await SSHTestSupport.waitForPort(port: port)

        _ = NSApplication.shared
        let transport = SSHTransport(configuration: SSHConfiguration(
            host: "127.0.0.1", port: port, subsystem: "srui",
            identityFile: userKey, knownHostsFile: knownHosts,
            strictHostKeyChecking: .yes, batchMode: true, connectTimeout: 5.0))
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(transport: transport, applier: applier,
                                           renderer: renderer, sessionId: "srtop")
        controller.attachRenderer(renderer)
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "native empty shell over SSH") {
            applier.lastAppliedRevision == Revision(1) && renderer.registry.handle(for: NodeId(5)) != nil
        }

        let surfaceHandle = try #require(renderer.registry.handle(for: NodeId(1)))
        let window = try #require(surfaceHandle.window)
        defer { window.close() }
        let headingHandle = try #require(renderer.registry.handle(for: NodeId(3)))
        let heading = try #require(headingHandle.view as? NSTextField)
        let status = try #require(renderer.registry.handle(for: NodeId(4))?.view as? NSTextField)
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        window.makeKeyAndOrderFront(nil)
        window.contentView?.layoutSubtreeIfNeeded()
        window.displayIfNeeded()
        #expect(window.isVisible)
        #expect(window.title == "Process Explorer")
        #expect(heading.stringValue == "Process Explorer")
        #expect(status.stringValue == "Read-only · Process collection not started")
        #expect(status.isEditable == false)
        #expect(table.numberOfRows == 0)
        #expect(table.tableColumns.map(\.title) == ["PID", "Name"])
        #expect(tableHandle.actionTrampoline == nil)
        let windowNumber = window.windowNumber
        try await Self.capture(window: window, name: "initial")

        #expect(kill(server.processIdentifier, SIGUSR1) == 0)
        try await AsyncTestSupport.eventually(timeout: .seconds(5), description: "title mutation over SSH") {
            applier.lastAppliedRevision == Revision(2) &&
                window.title == "Process Explorer — title fixture" &&
                heading.stringValue == "Process Explorer — title fixture"
        }
        #expect(renderer.registry.handle(for: NodeId(1)) === surfaceHandle)
        #expect(renderer.registry.handle(for: NodeId(3)) === headingHandle)
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(renderer.registry.handle(for: NodeId(1))?.window === window)
        #expect(window.windowNumber == windowNumber)
        #expect(window.isVisible)
        #expect(table.numberOfRows == 0)
        try await Self.capture(window: window, name: "updated")
        print("PX-001 native SSH evidence: revisions 1 -> 2; visible NSWindow \(windowNumber) retained; Surface, heading and Table handles retained; PID/Name columns; zero rows.")
        await controller.stop()
    }

    @MainActor
    private static func capture(window: NSWindow, name: String) async throws {
        guard let directory = ProcessInfo.processInfo.environment["PX001_EVIDENCE_DIR"] else { return }
        // Let AppKit finish its display cycle before capturing the retained native view.
        try await Task.sleep(for: .milliseconds(100))
        let output = URL(fileURLWithPath: directory)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let view = try #require(window.contentView)
        view.layoutSubtreeIfNeeded()
        let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let png = try #require(bitmap.representation(using: .png, properties: [:]))
        try png.write(to: output.appendingPathComponent("\(name).png"))
        let metadata: [String: Any] = [
            "title": window.title, "window_number": window.windowNumber,
            "visible": window.isVisible, "fixture": name, "transport": "real localhost SSH subsystem",
        ]
        try JSONSerialization.data(withJSONObject: metadata, options: [.prettyPrinted, .sortedKeys])
            .write(to: output.appendingPathComponent("\(name).json"))
    }
}
