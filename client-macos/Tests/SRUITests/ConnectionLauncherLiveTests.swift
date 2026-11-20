//
// ConnectionLauncherLiveTests.swift
// SRUITests
//
// PX-023: the Connect form's launcher path over a real localhost sshd and the real
// `srui-ssh-bridge` (§19, §19.1, §20.1, §25; D2 T38). Counter and srtop connect through the same
// `ConnectionManager.connect(_:)` and the same SSH configuration; an unknown or changed host key
// stops the attempt before the `srui` subsystem ever runs. Missing fixture binaries fail; nothing is
// skipped.
//

import AppKit
import Foundation
@testable import ConnectionManager
import RendererAppKit
import SemanticModel
import Testing
import TransportSSH

@MainActor
final class LauncherProbe {
    private(set) var requests: [ConnectionAttemptRequest] = []

    /// The production path with only the test's key and known_hosts file substituted: the request's
    /// configuration is what `ConnectionManager` built, and it must be the manager's one
    /// configuration for that entry, whatever app serves the subsystem.
    func make(identityFile: String, knownHostsFile: String, connectTimeout: TimeInterval = 10) -> ConnectionAttemptFactory {
        { [self] request in
            requests.append(request)
            #expect(request.configuration == ConnectionManager.sshConfiguration(for: request.connection))
            var configuration = request.configuration
            configuration.identityFile = identityFile
            configuration.knownHostsFile = knownHostsFile
            configuration.connectTimeout = connectTimeout
            return ConnectionManager.makeSessionControllerAttempt(
                request: request,
                transport: SSHTransport(configuration: configuration)
            )
        }
    }
}

struct LiveSSHD: @unchecked Sendable {
    enum KnownHosts: Sendable {
        case trusted
        case empty
        case spoofed
    }

    let directory: URL
    let port: UInt16
    let userKey: String
    let knownHosts: String
    let sshd: Process
    let server: Process?

    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()

    /// An authenticated localhost sshd whose `srui` subsystem is either the real bridge in front
    /// of `app` (its binary path and arguments) or, with no app, `/usr/bin/touch <marker>`, which
    /// records whether the subsystem ever ran.
    ///
    /// `hostileBanner` gives sshd a pre-authentication `Banner` that imitates OpenSSH's changed-key
    /// verdict for this port; `subsystemExits255` (without an app) makes the subsystem exit 255
    /// silently, so `ssh` exits 255 with no stream byte and no error of its own (PX-023 round 3).
    static func launch(
        prefix: String,
        knownHosts mode: KnownHosts,
        app: (binary: String, arguments: [String], refusal: [String])?,
        hostileBanner: Bool = false,
        subsystemExits255: Bool = false
    ) async throws -> LiveSSHD {
        let bridge = root.appendingPathComponent("server-rust/target/debug/srui-ssh-bridge").path
        let directory = try TestFixtureDirectory.make(prefix: prefix)
        let socket = directory.appendingPathComponent("s.sock").path
        let hostKey = directory.appendingPathComponent("host_key").path
        let otherKey = directory.appendingPathComponent("other_host_key").path
        let userKey = directory.appendingPathComponent("user_key").path
        let authorizedKeys = directory.appendingPathComponent("authorized_keys")
        let knownHosts = directory.appendingPathComponent("known_hosts").path
        let config = directory.appendingPathComponent("sshd_config")
        let server = app.map { _ in Process() }
        do {
            try SSHTestSupport.generateEd25519Key(at: hostKey)
            try SSHTestSupport.generateEd25519Key(at: userKey)
            try Data(contentsOf: URL(fileURLWithPath: "\(userKey).pub")).write(to: authorizedKeys)
            let port = SSHTestSupport.findFreePort()
            switch mode {
            case .trusted:
                try SSHTestSupport.writeKnownHosts(port: port, hostPublicKeyPath: hostKey, to: knownHosts)
            case .empty:
                try Data().write(to: URL(fileURLWithPath: knownHosts))
            case .spoofed:
                try SSHTestSupport.generateEd25519Key(at: otherKey)
                try SSHTestSupport.writeKnownHosts(port: port, hostPublicKeyPath: otherKey, to: knownHosts)
            }

            let subsystem: String
            if let app, let server {
                let binary = root.appendingPathComponent(app.binary).path
                try #require(FileManager.default.isExecutableFile(atPath: binary), "build \(app.binary) first")
                try #require(FileManager.default.isExecutableFile(atPath: bridge), "build srui-ssh-bridge first")
                try await FixtureBinaries.absorbFirstLaunchAssessment(binary, arguments: app.refusal)
                try await FixtureBinaries.absorbFirstLaunchAssessment(bridge, arguments: FixtureBinaries.bridgeRefusal)
                subsystem = "\(bridge) \(socket)"
                server.executableURL = URL(fileURLWithPath: binary)
                server.arguments = ["--socket", socket] + app.arguments
                server.standardOutput = FileHandle.nullDevice
                server.standardError = FileHandle.nullDevice
                try server.run()
                try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "\(app.binary) socket") {
                    FileManager.default.fileExists(atPath: socket)
                }
            } else if subsystemExits255 {
                // Read by `/bin/sh` rather than executed, so no first-exec assessment applies.
                let script = directory.appendingPathComponent("exit255.sh")
                try "exit 255\n".write(to: script, atomically: true, encoding: .utf8)
                subsystem = "/bin/sh \(script.path)"
            } else {
                subsystem = "/usr/bin/touch \(directory.appendingPathComponent("subsystem-ran").path)"
            }
            var bannerLine = ""
            if hostileBanner {
                let banner = directory.appendingPathComponent("banner")
                try Self.hostileBannerText(port: port).write(to: banner, atomically: true, encoding: .utf8)
                bannerLine = "Banner \(banner.path)"
            }

            try """
            Port \(port)
            ListenAddress 127.0.0.1
            HostKey \(hostKey)
            AuthorizedKeysFile \(authorizedKeys.path)
            StrictModes no
            UsePAM no
            PidFile \(directory.appendingPathComponent("sshd.pid").path)
            Subsystem srui \(subsystem)
            \(bannerLine)
            """.write(to: config, atomically: true, encoding: .utf8)
            let sshd = try SSHTestSupport.launchSSHD(configPath: config.path, hostKeyPath: hostKey, port: port)
            do {
                try await SSHTestSupport.waitForPort(port: port)
            } catch {
                SSHTestSupport.terminate(sshd)
                throw error
            }
            return LiveSSHD(directory: directory, port: port, userKey: userKey,
                            knownHosts: knownHosts, sshd: sshd, server: server)
        } catch {
            if let server, server.isRunning {
                kill(server.processIdentifier, SIGINT)
                server.waitUntilExit()
            }
            TestFixtureDirectory.release(directory)
            throw error
        }
    }

    /// A server-authored banner, sent before authentication, shaped exactly like OpenSSH's own
    /// changed-key refusal for this port, last line included.
    static let bannerMarker = "PX023 hostile banner marker"

    static func hostileBannerText(port: UInt16) -> String {
        """
        \(bannerMarker)
        @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
        @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @
        @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
        Host key for [127.0.0.1]:\(port) has changed and you have requested strict checking.
        Host key verification failed.

        """
    }

    var subsystemRan: Bool {
        FileManager.default.fileExists(atPath: directory.appendingPathComponent("subsystem-ran").path)
    }

    /// Tears the fixture down on a detached task. `shutDown()` waits with `Process.waitUntilExit()`,
    /// which services the calling thread's run loop: on the main actor that nests the main-actor
    /// executor's own run loop, and one PX-023 run parked there for 27 minutes with sshd already
    /// gone (`impl3-wedge-sample.txt`). The wait never runs on the main actor; whatever is still
    /// running when the test process exits is killed by `SSHDRegistry`'s exit hook.
    func shutDownOffMainActor() {
        let fixture = self
        Task.detached { fixture.shutDown() }
    }

    func shutDown() {
        if let server {
            if server.isRunning { kill(server.processIdentifier, SIGINT) }
            server.waitUntilExit()
        }
        SSHTestSupport.terminate(sshd)
        TestFixtureDirectory.release(directory)
    }
}

private func temporaryStore() -> (SavedConnectionStore, URL) {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("srui-px023-live-\(UUID().uuidString)", isDirectory: true)
    return (SavedConnectionStore(url: directory.appendingPathComponent("connections.json")), directory)
}

@MainActor
private func closeWindows(_ renderer: AppKitRenderer) {
    for window in NSApplication.shared.windows where renderer.registry.handle(for: NodeId(1))?.window === window {
        window.orderOut(nil)
        window.close()
    }
}

/// Serialized: each test runs its own sshd, server and native window on the one main actor.
@Suite("Connect form launcher over real SSH (PX-023, §19.1, §25)", .serialized, .timeLimit(.minutes(2)))
@MainActor
struct ConnectionLauncherLiveTests {
    @Test("a fresh known-host connection opens counter through the Connect form, and it is interactive")
    func counterThroughConnectForm() async throws {
        let live = try await LiveSSHD.launch(
            prefix: "px023-counter", knownHosts: .trusted,
            app: ("examples/counter/target/debug/counter", [], FixtureBinaries.counterRefusal))
        defer { live.shutDownOffMainActor() }
        let (store, storeDirectory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: storeDirectory) }
        _ = NSApplication.shared

        let probe = LauncherProbe()
        let manager = ConnectionManager(
            store: store, attemptFactory: probe.make(identityFile: live.userKey, knownHostsFile: live.knownHosts))
        // What a user types: user@host:port in the Host field, nothing else.
        let id = try #require(manager.connect(ConnectDraft(host: "\(NSUserName())@127.0.0.1:\(live.port)")))
        let request = try #require(probe.requests.first)
        #expect(request.configuration.buildArguments().suffix(8)
            == ["-p", "\(live.port)", "-l", NSUserName(), "-s", "--", "127.0.0.1", "srui"])
        let renderer = request.context.renderer
        defer { closeWindows(renderer) }

        try await AsyncTestSupport.observed("counter connected") {
            manager.status(for: id) == .connected || manager.alert != nil
        }
        #expect(manager.alert == nil, "\(manager.alert?.message ?? "")")
        #expect(manager.status(for: id) == .connected)
        #expect(manager.entries.first?.label == "\(NSUserName())@127.0.0.1:\(live.port)")
        try await AsyncTestSupport.eventually(description: "counter text rendered over SSH") {
            (renderer.registry.handle(for: NodeId(2))?.view as? NSTextField)?.stringValue == "Count: 0"
        }

        // The native button drives the remote app through the same connection.
        let button = try #require(renderer.registry.handle(for: NodeId(4))?.view as? NSButton)
        NativeActivation.click(button)
        try await AsyncTestSupport.eventually(description: "counter advanced over SSH") {
            (renderer.registry.handle(for: NodeId(2))?.view as? NSTextField)?.stringValue == "Count: 1"
        }
        #expect(manager.open(id: id))
        await manager.shutdown()
    }

    @Test("srtop opens through the same Connect form and SSH configuration")
    func srtopThroughConnectForm() async throws {
        let live = try await LiveSSHD.launch(
            prefix: "px023-srtop", knownHosts: .trusted,
            app: ("apps/srtop/target/debug/srtop", ["--fake-source"], FixtureBinaries.srtopRefusal))
        defer { live.shutDownOffMainActor() }
        let (store, storeDirectory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: storeDirectory) }
        _ = NSApplication.shared

        let probe = LauncherProbe()
        let manager = ConnectionManager(
            store: store, attemptFactory: probe.make(identityFile: live.userKey, knownHostsFile: live.knownHosts))
        // The separate-field form of the same endpoint.
        let id = try #require(manager.connect(ConnectDraft(
            label: "Explorer", host: "127.0.0.1", port: "\(live.port)", user: NSUserName())))
        let request = try #require(probe.requests.first)
        #expect(request.configuration.buildArguments().suffix(8)
            == ["-p", "\(live.port)", "-l", NSUserName(), "-s", "--", "127.0.0.1", "srui"])
        let renderer = request.context.renderer
        defer { closeWindows(renderer) }

        try await AsyncTestSupport.observed("srtop connected") {
            manager.status(for: id) == .connected || manager.alert != nil
        }
        #expect(manager.alert == nil, "\(manager.alert?.message ?? "")")
        try await AsyncTestSupport.eventually(description: "srtop fake rows rendered over SSH") {
            guard let scroll = renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView,
                  let table = scroll.documentView as? NSTableView else { return false }
            return table.numberOfRows == 3
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        #expect(window.title == "Process Explorer")
        #expect((renderer.registry.handle(for: NodeId(3))?.view as? NSTextField)?.stringValue == "Process Explorer")
        await manager.shutdown()
    }

    @Test("an unknown host key blocks with a diagnostic and adds nothing to known_hosts",
          arguments: [LiveSSHD.KnownHosts.empty, .spoofed])
    func untrustedHostKeyBlocks(_ mode: LiveSSHD.KnownHosts) async throws {
        let live = try await LiveSSHD.launch(prefix: "px023-hostkey", knownHosts: mode, app: nil)
        defer { live.shutDownOffMainActor() }
        let (store, storeDirectory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: storeDirectory) }
        let knownHostsBefore = try Data(contentsOf: URL(fileURLWithPath: live.knownHosts))

        let probe = LauncherProbe()
        let manager = ConnectionManager(
            store: store, attemptFactory: probe.make(identityFile: live.userKey, knownHostsFile: live.knownHosts))
        #expect(manager.connect(ConnectDraft(host: "127.0.0.1:\(live.port)", user: NSUserName())) != nil)
        let request = try #require(probe.requests.first)

        try await AsyncTestSupport.observed("host-key alert") { manager.alert != nil }
        let alert = try #require(manager.alert)
        #expect(alert.kind == .hostKeyRejected, "\(alert.message)")
        #expect(alert.title == "Host Key Not Trusted")
        #expect(alert.message.hasPrefix("SRUI did not connect to \(NSUserName())@127.0.0.1:\(live.port)."))
        switch mode {
        case .empty:
            #expect(alert.message.contains("Your known_hosts file has no host key for [127.0.0.1]:\(live.port)"))
            #expect(alert.message.contains("host key is known for [127.0.0.1]:\(live.port)"))
        case .spoofed:
            #expect(alert.message.contains("does not match the key your known_hosts file holds for [127.0.0.1]:\(live.port)"))
            #expect(alert.message.contains("REMOTE HOST IDENTIFICATION HAS CHANGED"))
        case .trusted:
            Issue.record("not a host-key case")
        }
        #expect(alert.message.contains("Host key verification failed."))
        Self.recordEvidence(alert, mode: mode)

        // Nothing connected: the draft is forgotten, no state was applied or rendered, the
        // subsystem never ran, and known_hosts is byte-for-byte unchanged.
        try await AsyncTestSupport.observed("failed draft forgotten") { manager.entries.isEmpty }
        #expect(request.context.applier.lastAppliedRevision == Revision(0))
        #expect(request.context.renderer.registry.isEmpty)
        #expect(live.subsystemRan == false)
        #expect(try Data(contentsOf: URL(fileURLWithPath: live.knownHosts)) == knownHostsBefore)
        #expect(probe.requests.count == 1)
        await manager.shutdown()
    }

    enum BannerCase: String, Sendable, CaseIterable {
        /// The key is trusted, the client's key is not authorized: the banner is printed, then
        /// `ssh` ends with its own "Permission denied (…)." line.
        case authenticationFails
        /// The key is trusted and authentication succeeds; the subsystem exits 255 silently, so
        /// the banner is the last thing on `ssh`'s stderr: the documented residual.
        case subsystemExits255
        /// known_hosts has no key for the server: key exchange never completes, so the banner,
        /// a userauth message, is never sent.
        case unknownKey
        /// known_hosts holds another key for the server: the same.
        case changedKey
    }

    @Test("a server's hostile Banner never raises the host-key alert for a trusted key it cannot reach past",
          arguments: BannerCase.allCases)
    func hostileBanner(_ bannerCase: BannerCase) async throws {
        let mode: LiveSSHD.KnownHosts = switch bannerCase {
        case .authenticationFails, .subsystemExits255: .trusted
        case .unknownKey: .empty
        case .changedKey: .spoofed
        }
        let live = try await LiveSSHD.launch(
            prefix: "px023-banner", knownHosts: mode, app: nil,
            hostileBanner: true, subsystemExits255: bannerCase == .subsystemExits255)
        defer { live.shutDownOffMainActor() }
        let (store, storeDirectory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: storeDirectory) }
        let knownHostsBefore = try Data(contentsOf: URL(fileURLWithPath: live.knownHosts))
        var identity = live.userKey
        if bannerCase == .authenticationFails {
            identity = live.directory.appendingPathComponent("stranger_key").path
            try SSHTestSupport.generateEd25519Key(at: identity)
        }

        let probe = LauncherProbe()
        let manager = ConnectionManager(
            store: store, attemptFactory: probe.make(identityFile: identity, knownHostsFile: live.knownHosts))
        #expect(manager.connect(ConnectDraft(host: "127.0.0.1:\(live.port)", user: NSUserName())) != nil)
        let request = try #require(probe.requests.first)
        #expect(request.configuration.buildArguments().contains("LogLevel=INFO"))

        try await AsyncTestSupport.observed("connection alert") { manager.alert != nil }
        let alert = try #require(manager.alert)
        Self.recordEvidence(alert, name: "banner-\(bannerCase.rawValue)")
        switch bannerCase {
        case .authenticationFails:
            // At INFO the banner is printed, but ssh's own line ends the stream.
            #expect(alert.kind == .connectionFailed, "\(alert.kind): \(alert.message)")
            #expect(alert.title == "Connection Failed")
            #expect(alert.message.contains("ssh-keygen") == false)
            #expect(Self.lastLine(alert.message).hasSuffix("Permission denied (publickey,password,keyboard-interactive)."),
                    "\(alert.message)")
        case .unknownKey, .changedKey:
            // Protocol order: the banner follows key exchange, which a refused key never
            // completes, so ssh's real verdict is classified and the banner is nowhere in it.
            #expect(alert.kind == .hostKeyRejected, "\(alert.kind): \(alert.message)")
            #expect(alert.message.contains(LiveSSHD.bannerMarker) == false, "\(alert.message)")
        case .subsystemExits255:
            // The documented residual (PX-023 round 4): a trusted server the user authenticated
            // to sends the banner, then its subsystem exits 255 without a byte or a word. At
            // INFO nothing of ssh's own follows the banner, so its last line can be the verdict.
            // Only safety is asserted: nothing connected and nothing was trusted. Which alert
            // appears is recorded in the evidence, not pinned.
            #expect(manager.status(for: request.connection.id) != .connected)
        }
        #expect(manager.entries.isEmpty, "a failed first connection is forgotten")
        #expect(try Data(contentsOf: URL(fileURLWithPath: live.knownHosts)) == knownHostsBefore)
        await manager.shutdown()
    }

    static func lastLine(_ text: String) -> String {
        text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .last { $0.isEmpty == false } ?? ""
    }

    @Test("ssh's own connection diagnostics reach the alert: a refused connection says so")
    func connectionRefusedKeepsItsDiagnostic() async throws {
        let (store, storeDirectory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: storeDirectory) }
        let directory = try TestFixtureDirectory.make(prefix: "px023-refused")
        defer { TestFixtureDirectory.release(directory) }
        let knownHosts = directory.appendingPathComponent("known_hosts").path
        try Data().write(to: URL(fileURLWithPath: knownHosts))
        // Bound and released at once: nothing listens there.
        let port = SSHTestSupport.findFreePort()

        let probe = LauncherProbe()
        let manager = ConnectionManager(
            store: store,
            attemptFactory: probe.make(identityFile: directory.appendingPathComponent("absent_key").path,
                                       knownHostsFile: knownHosts))
        #expect(manager.connect(ConnectDraft(host: "127.0.0.1:\(port)", user: NSUserName())) != nil)
        try await AsyncTestSupport.observed("refused alert") { manager.alert != nil }
        let alert = try #require(manager.alert)
        #expect(alert.kind == .connectionFailed, "\(alert.message)")
        #expect(alert.message.contains("ssh: connect to host 127.0.0.1 port \(port): Connection refused"), "\(alert.message)")
        Self.recordEvidence(alert, name: "connection-refused")
        await manager.shutdown()
    }

    /// Writes the real alert text to `PX023_EVIDENCE_DIR` when set, for the completion note, named
    /// with `PX023_EVIDENCE_PREFIX` (default `px023`) so separate runs keep separate files.
    private static func recordEvidence(_ alert: ConnectionAlert, mode: LiveSSHD.KnownHosts) {
        recordEvidence(alert, name: "hostkey-alert-\(mode)")
    }

    private static func recordEvidence(_ alert: ConnectionAlert, name: String) {
        let environment = ProcessInfo.processInfo.environment
        guard let directory = environment["PX023_EVIDENCE_DIR"] else { return }
        let prefix = environment["PX023_EVIDENCE_PREFIX"] ?? "px023"
        let file = URL(fileURLWithPath: directory).appendingPathComponent("\(prefix)-\(name).txt")
        try? "\(alert.title)\n\n\(alert.message)\n".write(to: file, atomically: true, encoding: .utf8)
    }
}
