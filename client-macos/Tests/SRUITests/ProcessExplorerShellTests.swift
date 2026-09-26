// PX-001/PX-002/PX-004: real non-PTY SSH launch through the unchanged generic client (§§8, 12, 22, 29).
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
    @Test(.serialized, .timeLimit(.minutes(1)), arguments: [false, true])
    @MainActor
    func shellOverSSHRetainsNativeHandlesAfterTitleFixture(fakeSource: Bool) async throws {
        let harness = try await Self.launch(
            arguments: ["--smoke-fixture"] + (fakeSource ? ["--fake-source"] : []))
        defer { Self.shutDown(harness) }

        _ = NSApplication.shared
        let transport = SSHTransport(configuration: harness.configuration)
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
        defer { window.orderOut(nil); window.close() }
        let headingHandle = try #require(renderer.registry.handle(for: NodeId(3)))
        let heading = try #require(headingHandle.view as? NSTextField)
        let status = try #require(renderer.registry.handle(for: NodeId(4))?.view as? NSTextField)
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        // The session already ordered the surface in; re-order it under the host's own
        // presentation policy rather than forcing it front over the developer's desktop.
        SurfacePresentation.forHostApplication().present(window)
        window.contentView?.layoutSubtreeIfNeeded()
        window.displayIfNeeded()
        #expect(window.isVisible)
        #expect(window.title == "Process Explorer")
        #expect(heading.stringValue == "Process Explorer")
        #expect(status.stringValue == (fakeSource ? "Read-only · Fake process snapshot" : "Read-only · Process collection not started"))
        #expect(status.isEditable == false)
        #expect(table.numberOfRows == (fakeSource ? 3 : 0))
        #expect(table.tableColumns.map(\.title) == ["PID", "Name"])
        #expect(tableHandle.actionTrampoline == nil)
        if fakeSource {
            let expected = [["4101", "worker"], ["4102", "worker"], ["Unavailable", "helper"]]
            for row in 0..<3 {
                for column in 0..<2 {
                    let cell = try #require(table.view(atColumn: column, row: row, makeIfNecessary: true) as? NSTextField)
                    #expect(cell.stringValue == expected[row][column])
                }
            }
        }
        let windowNumber = window.windowNumber
        try await Self.capture(window: window, name: fakeSource ? "fake-initial" : "initial")

        #expect(kill(harness.server.processIdentifier, SIGUSR1) == 0)
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
        #expect(table.numberOfRows == (fakeSource ? 3 : 0))
        try await Self.capture(window: window, name: fakeSource ? "fake-updated" : "updated")
        print("PX-001/PX-002 native SSH evidence: fakeSource=\(fakeSource); revisions 1 -> 2; visible NSWindow \(windowNumber) retained; Surface, heading and Table handles retained; PID/Name columns; rows=\(table.numberOfRows).")
        await controller.stop()
    }

    /// PX-004: the scripted fixture sequence refreshes the same native table in
    /// place — one insertion, one deletion, one rename, a failed scan that keeps
    /// the last-known rows, and a tick that publishes nothing at all.
    @Test(.serialized, .timeLimit(.minutes(2)))
    @MainActor
    func scriptedRefreshUpdatesTheSameNativeTableInPlace() async throws {
        let intervalMilliseconds = 500
        let harness = try await Self.launch(arguments: [
            "--fake-sequence", "--refresh-interval-ms", "\(intervalMilliseconds)",
        ])
        defer { Self.shutDown(harness) }

        _ = NSApplication.shared
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(transport: SSHTransport(configuration: harness.configuration),
                                           applier: applier, renderer: renderer, sessionId: "srtop")
        controller.attachRenderer(renderer)
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "native process table over SSH") {
            applier.lastAppliedRevision.value >= 1 && renderer.registry.handle(for: NodeId(5)) != nil
        }
        let surfaceHandle = try #require(renderer.registry.handle(for: NodeId(1)))
        let window = try #require(surfaceHandle.window)
        defer { window.orderOut(nil); window.close() }
        let statusField = try #require(renderer.registry.handle(for: NodeId(4))?.view as? NSTextField)
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        SurfacePresentation.forHostApplication().present(window)
        window.contentView?.layoutSubtreeIfNeeded()
        let windowNumber = window.windowNumber

        // Sample far faster than the server polls, so no published state can
        // pass unobserved.
        var trace: [Observation] = []
        let clock = ContinuousClock()
        let start = clock.now
        let deadline = start.advanced(by: .seconds(20))
        var stamps: [Duration] = []
        while clock.now < deadline {
            window.contentView?.layoutSubtreeIfNeeded()
            // The renderer applies a transaction just after the applier does, so
            // a sample taken between the two would pair a new revision with the
            // previous rows. Only a sample where the native controls already
            // show what the client holds is a published state — which is also
            // the assertion that the native table mirrors the model.
            let observation = Observation(revision: applier.lastAppliedRevision.value,
                                          status: statusField.stringValue,
                                          rows: Self.nativeRows(table),
                                          itemIDs: Self.modelItemIDs(applier))
            guard observation.rows == Self.modelRows(applier),
                  observation.status == Self.modelStatus(applier) else {
                try await Task.sleep(for: .milliseconds(20))
                continue
            }
            if trace.last != observation {
                trace.append(observation)
                stamps.append(start.duration(to: clock.now))
            }
            // One full cycle is five ticks; two cycles prove the script repeats.
            if trace.filter({ $0.rows == Self.initialRows }).count >= 3 { break }
            try await Task.sleep(for: .milliseconds(20))
        }

        let settledRows = [["4101", "worker"], ["4103", "helper-tool"], ["4104", "builder"]]
        let normalStatus = "Read-only · Fake process sequence"
        #expect(trace.contains { $0.rows == Self.initialRows && $0.status == normalStatus })
        #expect(trace.contains { $0.rows == settledRows && $0.status == normalStatus })
        // A failed scan keeps every row it had and says so.
        let failed = try #require(trace.first { $0.status.contains("retained from an earlier scan") })
        #expect(failed.rows == settledRows, "a failed scan must not empty the table")
        #expect(failed.status == "\(normalStatus) · incomplete scan · process list unavailable: permission denied · 3 rows retained from an earlier scan")
        // Recovery converges back onto the same rows.
        let recoveredIndex = try #require(trace.firstIndex { $0.status.contains("retained") }) + 1
        try #require(recoveredIndex < trace.count)
        #expect(trace[recoveredIndex].status == normalStatus)
        #expect(trace[recoveredIndex].rows == settledRows)
        #expect(trace[recoveredIndex].itemIDs == failed.itemIDs, "recovery must move no row")

        // The process that never changed keeps one row identity throughout, and
        // so does the renamed one.
        let unchanged = try #require(trace.first { $0.rows.first?.first == "4101" }?.itemIDs.first)
        for observation in trace where observation.rows.first?.first == "4101" {
            #expect(observation.itemIDs.first == unchanged, "an unchanged row changed identity")
        }
        let renamedBefore = try #require(trace.first { $0.rows == Self.initialRows }?.itemIDs.last)
        let renamedAfter = try #require(trace.first { $0.rows == settledRows }?.itemIDs[1])
        #expect(renamedBefore == renamedAfter, "a rename must keep the row identity")

        // One cycle is five ticks and exactly four transactions: the tick whose
        // snapshot repeated the previous one published nothing at all.
        let cycleStarts = trace.indices.filter { trace[$0].rows == Self.initialRows }
        try #require(cycleStarts.count >= 2)
        let cycle = trace[cycleStarts[1]].revision - trace[cycleStarts[0]].revision
        #expect(cycle == 4, "a repeated snapshot must cost no transaction")

        // Everything above happened inside the native controls built once.
        #expect(renderer.registry.handle(for: NodeId(1)) === surfaceHandle)
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(tableHandle.view as? NSScrollView === scroll)
        #expect(scroll.documentView as? NSTableView === table)
        #expect(renderer.registry.handle(for: NodeId(4))?.view as? NSTextField === statusField)
        #expect(window.windowNumber == windowNumber)
        #expect(window.isVisible)
        #expect(table.tableColumns.map(\.title) == ["PID", "Name"])
        #expect(tableHandle.actionTrampoline == nil)
        try await Self.capture(window: window, name: "sequence-settled")
        let elapsed = stamps.isEmpty ? Duration.zero : stamps[stamps.count - 1]
        print("""
        PX-004 native SSH evidence: interval=\(intervalMilliseconds)ms window \(windowNumber) retained; \
        observed \(trace.count) published states in \(elapsed); revisions per five-tick cycle=\(cycle); \
        unchanged row ItemId=\(unchanged); states=\(trace.map { "\($0.revision):\($0.rows.map { $0[0] }.joined(separator: ","))" })
        """)
        await controller.stop()
    }

    /// What a client actually holds and shows at one moment.
    private struct Observation: Equatable {
        let revision: UInt64
        let status: String
        let rows: [[String]]
        let itemIDs: [UInt64]
    }

    private static let initialRows = [["4101", "worker"], ["4102", "worker"], ["4103", "helper"]]

    @MainActor
    private static func nativeRows(_ table: NSTableView) -> [[String]] {
        (0..<table.numberOfRows).map { row in
            (0..<table.tableColumns.count).map { column in
                (table.view(atColumn: column, row: row, makeIfNecessary: true) as? NSTextField)?
                    .stringValue ?? ""
            }
        }
    }

    /// Stable item identities held by the client, in published order.
    @MainActor
    private static func modelItemIDs(_ applier: TransactionApplier) -> [UInt64] {
        guard let model = applier.store.getModel(ModelId(1)) else { return [] }
        return model.items.sorted { $0.key < $1.key }.map { $0.value.itemID.value }
    }

    /// The row cells the client holds, in published order.
    @MainActor
    private static func modelRows(_ applier: TransactionApplier) -> [[String]] {
        guard let model = applier.store.getModel(ModelId(1)) else { return [] }
        return model.items.sorted { $0.key < $1.key }.map { item in
            guard case .list(let cells) = item.value.value else { return [] }
            return cells.map { cell in
                switch cell {
                case .unsignedInt(let number): return String(number)
                case .string(let text): return text
                default: return String(describing: cell)
                }
            }
        }
    }

    /// The status text the client holds.
    @MainActor
    private static func modelStatus(_ applier: TransactionApplier) -> String {
        guard case .string(let text)? = applier.store.getNode(NodeId(4))?.getProperty(.text) else {
            return ""
        }
        return text
    }

    private struct Harness {
        let temp: URL
        let server: Process
        let sshd: Process
        let configuration: SSHConfiguration
    }

    /// Builds the app, an ephemeral authenticated localhost sshd and a transport
    /// configuration for it. Missing prerequisites fail; nothing is skipped.
    @MainActor
    private static func launch(arguments: [String]) async throws -> Harness {
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
        let socket = temp.appendingPathComponent("s.sock").path
        let hostKey = temp.appendingPathComponent("host_key").path
        let userKey = temp.appendingPathComponent("user_key").path
        let authorizedKeys = temp.appendingPathComponent("authorized_keys")
        let knownHosts = temp.appendingPathComponent("known_hosts").path
        let config = temp.appendingPathComponent("sshd_config")
        let server = Process()
        do {
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

            server.executableURL = appBinary
            server.arguments = ["--socket", socket] + arguments
            // A test process must never inherit the test binary's stdio.
            server.standardOutput = FileHandle.nullDevice
            server.standardError = FileHandle.nullDevice
            try server.run()
            try await AsyncTestSupport.eventually(timeout: .seconds(5), description: "srtop private socket") {
                FileManager.default.fileExists(atPath: socket)
            }
            let sshd = try SSHTestSupport.launchSSHD(configPath: config.path, hostKeyPath: hostKey, port: port)
            do {
                try await SSHTestSupport.waitForPort(port: port)
            } catch {
                SSHTestSupport.terminate(sshd)
                throw error
            }
            return Harness(temp: temp, server: server, sshd: sshd,
                           configuration: SSHConfiguration(
                               host: "127.0.0.1", port: port, subsystem: "srui",
                               identityFile: userKey, knownHostsFile: knownHosts,
                               strictHostKeyChecking: .yes, batchMode: true, connectTimeout: 5.0))
        } catch {
            if server.isRunning {
                kill(server.processIdentifier, SIGINT)
                server.waitUntilExit()
            }
            try? FileManager.default.removeItem(at: temp)
            throw error
        }
    }

    private static func shutDown(_ harness: Harness) {
        if harness.server.isRunning { kill(harness.server.processIdentifier, SIGINT) }
        harness.server.waitUntilExit()
        SSHTestSupport.terminate(harness.sshd)
        try? FileManager.default.removeItem(at: harness.temp)
    }

    @MainActor
    private static func capture(window: NSWindow, name: String) async throws {
        guard let directory = (ProcessInfo.processInfo.environment["PX002_EVIDENCE_DIR"] ?? ProcessInfo.processInfo.environment["PX001_EVIDENCE_DIR"]) else { return }
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
