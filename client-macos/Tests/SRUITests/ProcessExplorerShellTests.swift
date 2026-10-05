// PX-001/PX-002/PX-004/PX-005/PX-006: real non-PTY SSH launch through the unchanged generic client (§§8, 12, 22, 29).
import Testing
import Foundation
import AppKit
import Darwin
import SemanticModel
import Protocol
@testable import Session
import TransportSSH
import RendererAppKit

/// Serialized as a suite, not per test: `.serialized` on a non-parameterized test function is a
/// no-op (the compiler warns), and each test here launches its own sshd, srtop and NSWindow. Two of
/// them at once triples that load on the one main actor every test in this process shares, and
/// PX-004 samples native controls on a 500 ms tick schedule while it runs. Serializing the three
/// cases costs about one second of wall clock.
@Suite(.serialized)
struct ProcessExplorerShellTests {
    @Test(.timeLimit(.minutes(1)), arguments: [false, true])
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
        #expect(table.tableColumns.map(\.title) == ["PID", "Name", "Resident", "CPU (100% = 1 CPU)"])
        #expect(tableHandle.actionTrampoline == nil)
        if fakeSource {
            // PX-005: the server published the unit, so the generic client shows
            // it verbatim — a truncated value, a known zero, and a metric this
            // fixture's scan was denied, each distinct in the native cell.
            // PX-006: likewise for CPU — past 100% of one CPU, a measured zero,
            // and a first sample that is visibly warming up.
            let expected = [
                ["4101", "worker", "1.1 MiB", "250.0%"],
                ["4102", "worker", "0 B", "0.0%"],
                ["Unavailable", "helper", "Denied", "Warming up"],
            ]
            for row in 0..<3 {
                for column in 0..<4 {
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
        print("PX-001/PX-002/PX-005/PX-006 native SSH evidence: fakeSource=\(fakeSource); revisions 1 -> 2; visible NSWindow \(windowNumber) retained; Surface, heading and Table handles retained; PID/Name/Resident/CPU columns; rows=\(table.numberOfRows); cells=\(Self.nativeRows(table)).")
        await controller.stop()
    }

    /// PX-004: the scripted fixture sequence refreshes the same native table in
    /// place — one insertion, one deletion, one rename, a failed scan that keeps
    /// the last-known rows, and a tick that publishes nothing at all.
    @Test(.timeLimit(.minutes(2)))
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

        // Record one entry per rendered transaction from the renderer's own completion hook
        // instead of sampling the controls. A poll loop only sees what it happens to be
        // scheduled for: on a contended machine (3-core CI, the whole suite in parallel) a
        // 20ms sleep can resume after a whole 500ms tick, so states the assertions below name
        // — the failed scan and its recovery — can pass unobserved even though the client
        // published them. The hook runs after the transaction is applied *and* rendered and
        // before the next one is, so no published state can be missed at any machine speed.
        // A session that fails mid-run must name itself in the timeout below rather than look
        // like a slow machine.
        let sessionFailure = ManagedAtomic<String?>(nil)
        controller.onFailure = { failure in sessionFailure.store(String(describing: failure)) }
        let recorder = PublishedStateRecorder(applier: applier, window: window,
                                              statusField: statusField, table: table)
        controller.rendererDidRenderInterceptorForTesting = { [recorder] in
            await recorder.record()
        }
        defer { controller.rendererDidRenderInterceptorForTesting = nil }

        // Wait for the states the assertions read rather than for a cycle count: two cycle
        // starts prove the five-tick script repeats, and the failed scan needs the state that
        // follows it to show the recovery. Sleep between checks rather than spinning the main
        // actor - recording is event-driven, so polling only decides when to stop, and a
        // multi-second main-actor spin would delay the very renders being recorded.
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: .seconds(60))
        while Self.coversTwoScriptedCycles(recorder.states) == false {
            guard clock.now < deadline else {
                throw ScriptedSequenceIncomplete(
                    description: "the scripted sequence did not complete in 60s; recorded "
                        + "\(recorder.states.count) published states: "
                        + "\(recorder.states.map { "\($0.revision):\($0.status)" }); "
                        + "revision=\(applier.lastAppliedRevision.value) "
                        + "serverRunning=\(harness.server.isRunning) "
                        + "sshdRunning=\(harness.sshd.isRunning) "
                        + "sessionFailure=\(sessionFailure.load() ?? "none")"
                )
            }
            try await Task.sleep(for: .milliseconds(20))
        }
        controller.rendererDidRenderInterceptorForTesting = nil
        let trace = recorder.states
        let stamps = recorder.stamps
        // Every entry was read after its own transaction had rendered, so the native controls
        // must already show what the client holds — this is the assertion that the native table
        // mirrors the model, not a filter that drops the states where it does not.
        #expect(recorder.divergences.isEmpty, "native controls diverged from the client's model")

        let settledRows = Self.settledRows
        let normalStatus = Self.normalStatus
        #expect(trace.contains { $0.rows == Self.initialRows && $0.status == normalStatus })
        #expect(trace.contains { $0.rows == settledRows && $0.status == normalStatus })
        // A failed scan keeps every row it had and says so.
        let failed = try #require(trace.first { $0.status.contains(Self.retainedRowsMarker) })
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
        #expect(table.tableColumns.map(\.title) == ["PID", "Name", "Resident", "CPU (100% = 1 CPU)"])
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
    fileprivate struct Observation: Equatable, Sendable {
        let revision: UInt64
        let status: String
        let rows: [[String]]
        let itemIDs: [UInt64]
    }

    // PX-005: every scripted row carries its resident cell, fixed per process so
    // that a repeated snapshot stays byte-identical and still publishes nothing.
    // PX-006: the CPU cell likewise, one per CPU state the script states.
    private static let initialRows = [
        ["4101", "worker", "2.0 MiB", "50.0%"],
        ["4102", "worker", "1023 B", "0.0%"],
        ["4103", "helper", "Unavailable", "Unavailable"],
    ]
    private static let settledRows = [
        ["4101", "worker", "2.0 MiB", "50.0%"],
        ["4103", "helper-tool", "Unavailable", "Unavailable"],
        ["4104", "builder", "5.0 GiB", "Warming up"],
    ]
    private static let normalStatus = "Read-only · Fake process sequence"
    private static let retainedRowsMarker = "retained from an earlier scan"

    /// Whether the recorded states cover every state the PX-004 assertions read.
    private static func coversTwoScriptedCycles(_ trace: [Observation]) -> Bool {
        guard trace.filter({ $0.rows == initialRows && $0.status == normalStatus }).count >= 2,
              trace.contains(where: { $0.rows == settledRows && $0.status == normalStatus }),
              let failed = trace.firstIndex(where: { $0.status.contains(retainedRowsMarker) }),
              failed + 1 < trace.count else {
            return false
        }
        return true
    }

    @MainActor
    fileprivate static func nativeRows(_ table: NSTableView) -> [[String]] {
        (0..<table.numberOfRows).map { row in
            (0..<table.tableColumns.count).map { column in
                (table.view(atColumn: column, row: row, makeIfNecessary: true) as? NSTextField)?
                    .stringValue ?? ""
            }
        }
    }

    /// Stable item identities held by the client, in published order.
    @MainActor
    fileprivate static func modelItemIDs(_ applier: TransactionApplier) -> [UInt64] {
        guard let model = applier.store.getModel(ModelId(1)) else { return [] }
        return model.items.sorted { $0.key < $1.key }.map { $0.value.itemID.value }
    }

    /// The row cells the client holds, in published order.
    @MainActor
    fileprivate static func modelRows(_ applier: TransactionApplier) -> [[String]] {
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
    fileprivate static func modelStatus(_ applier: TransactionApplier) -> String {
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

private struct ScriptedSequenceIncomplete: Error, CustomStringConvertible {
    let description: String
}

/// Every state the client published, one entry per rendered transaction.
///
/// `rendererDidRenderInterceptorForTesting` is the only vantage point from which a published
/// state is observable without sampling: it runs once per transaction, after that transaction
/// has been applied and rendered and before the next one is, so the record is complete and
/// ordered however slow or contended the machine is.
@MainActor
private final class PublishedStateRecorder {
    private(set) var states: [ProcessExplorerShellTests.Observation] = []
    private(set) var stamps: [Duration] = []
    /// Revisions where the native controls did not show what the client held after rendering.
    private(set) var divergences: [String] = []

    private let applier: TransactionApplier
    private let window: NSWindow
    private let statusField: NSTextField
    private let table: NSTableView
    private let clock = ContinuousClock()
    private let start: ContinuousClock.Instant

    init(applier: TransactionApplier, window: NSWindow, statusField: NSTextField, table: NSTableView) {
        self.applier = applier
        self.window = window
        self.statusField = statusField
        self.table = table
        self.start = clock.now
    }

    func record() {
        window.contentView?.layoutSubtreeIfNeeded()
        let observation = ProcessExplorerShellTests.Observation(
            revision: applier.lastAppliedRevision.value,
            status: statusField.stringValue,
            rows: ProcessExplorerShellTests.nativeRows(table),
            itemIDs: ProcessExplorerShellTests.modelItemIDs(applier)
        )
        let modelRows = ProcessExplorerShellTests.modelRows(applier)
        let modelStatus = ProcessExplorerShellTests.modelStatus(applier)
        if observation.rows != modelRows || observation.status != modelStatus {
            divergences.append(
                "rev \(observation.revision): native \(observation.rows)/\(observation.status)"
                    + " vs model \(modelRows)/\(modelStatus)"
            )
        }
        guard states.last != observation else { return }
        states.append(observation)
        stamps.append(start.duration(to: clock.now))
    }
}
