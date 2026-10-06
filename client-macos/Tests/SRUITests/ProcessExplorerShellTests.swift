// PX-001/PX-002/PX-004/PX-005/PX-006/PX-007/PX-008: real non-PTY SSH launch through the unchanged generic client (§§4, 8, 12, 19.2, 22, 29, 31).
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
        // PX-007: the system summary, rendered by the generic client as native labels and
        // progress bars between the status line and the table.
        let summaryHandles = try Self.summaryHandles(renderer)
        let summaryLines = try Self.summaryLines(renderer)
        let summaryBars = try Self.summaryBars(renderer)
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
        // PX-007: every summary line as the server worded it, and each bar filled to the share
        // its line prints — or hidden, carrying no value, where there is no share: a host with
        // no swap, and an empty shell that has sampled nothing.
        #expect(summaryLines.map(\.stringValue) == (fakeSource ? Self.fakeSummary : Self.unsampledSummary))
        #expect(summaryBars.map { $0.accessibilityLabel() } == ["Overall CPU", "Memory used", "Swap used"])
        if fakeSource {
            #expect(abs(summaryBars[0].doubleValue - 0.312) < 1e-9)
            #expect(abs(summaryBars[1].doubleValue - 0.25) < 1e-9)
            #expect(summaryBars.map(\.alphaValue) == [1, 1, 0])
        } else {
            #expect(summaryBars.map(\.alphaValue) == [0, 0, 0])
        }
        #expect(summaryBars.allSatisfy { !$0.isIndeterminate })
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
        // The summary's native controls are the same objects, showing the same lines.
        for (handle, id) in zip(summaryHandles, Self.summaryNodeIDs) {
            #expect(renderer.registry.handle(for: NodeId(id)) === handle, "summary node \(id) was rebuilt")
        }
        #expect(summaryLines.map(\.stringValue) == (fakeSource ? Self.fakeSummary : Self.unsampledSummary))
        try await Self.capture(window: window, name: fakeSource ? "fake-updated" : "updated")
        print("PX-001/PX-002/PX-005/PX-006/PX-007 native SSH evidence: fakeSource=\(fakeSource); revisions 1 -> 2; visible NSWindow \(windowNumber) retained; Surface, heading, Table and \(summaryHandles.count) summary handles retained; PID/Name/Resident/CPU columns; rows=\(table.numberOfRows); cells=\(Self.nativeRows(table)); summary=\(summaryLines.map(\.stringValue)); bars=\(summaryBars.map { ($0.doubleValue, $0.alphaValue) }).")
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
        let summaryHandles = try Self.summaryHandles(renderer)
        let summaryLines = try Self.summaryLines(renderer)
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
                                              statusField: statusField, table: table,
                                              summaryLines: summaryLines)
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

        // PX-007: every published state shows the script's fixed figures, and the freshness
        // line its sample time. The failed scan keeps those figures under a collector error
        // that names the last successful sample — the step before it, two seconds before the
        // step that recovers — and the recovery shows its own time again.
        for observation in trace {
            #expect(Array(observation.summary.dropLast()) == Self.scriptedFigures, "rev \(observation.revision)")
        }
        let failedFreshness = try #require(failed.summary.last)
        let collectorError = "Collector error: could not list processes (permission denied) · last successful sample: "
        #expect(failedFreshness.hasPrefix(collectorError), "\(failedFreshness)")
        #expect(failedFreshness.hasSuffix(" UTC (server clock) · source: fake-process-sequence-v1"), "\(failedFreshness)")
        let recoveredFreshness = try #require(trace[recoveredIndex].summary.last)
        #expect(recoveredFreshness.hasPrefix("Last successful sample: "), "\(recoveredFreshness)")
        let failedSecond = try #require(Self.sampleSecond(failedFreshness))
        let recoveredSecond = try #require(Self.sampleSecond(recoveredFreshness))
        #expect(recoveredSecond - failedSecond == 2, "\(failedFreshness) / \(recoveredFreshness)")
        #expect(trace[recoveredIndex].revision == failed.revision + 1)

        // The process that never changed keeps one row identity throughout, and
        // so does the renamed one.
        let unchanged = try #require(trace.first { $0.rows.first?.first == "4101" }?.itemIDs.first)
        for observation in trace where observation.rows.first?.first == "4101" {
            #expect(observation.itemIDs.first == unchanged, "an unchanged row changed identity")
        }
        let renamedBefore = try #require(trace.first { $0.rows == Self.initialRows }?.itemIDs.last)
        let renamedAfter = try #require(trace.first { $0.rows == settledRows }?.itemIDs[1])
        #expect(renamedBefore == renamedAfter, "a rename must keep the row identity")

        // One cycle is five ticks and exactly five transactions. PX-004 counted four, because
        // the tick whose snapshot repeated the previous one published nothing; since PX-007 the
        // freshness line shows each step's sample time, so that tick publishes exactly that
        // one line, and no row and no status move for it.
        let cycleStarts = Self.cycleStarts(trace)
        try #require(cycleStarts.count >= 2)
        let cycle = trace[cycleStarts[1]].revision - trace[cycleStarts[0]].revision
        #expect(cycle == 5, "one transaction per tick: the repeated snapshot costs only its sample time")
        let repeated = cycleStarts[0] + 1
        try #require(repeated < trace.count)
        #expect(trace[repeated].rows == Self.initialRows && trace[repeated].status == normalStatus)
        #expect(trace[repeated].summary.last != trace[cycleStarts[0]].summary.last, "the repeated tick shows its own time")

        // Everything above happened inside the native controls built once.
        #expect(renderer.registry.handle(for: NodeId(1)) === surfaceHandle)
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(tableHandle.view as? NSScrollView === scroll)
        #expect(scroll.documentView as? NSTableView === table)
        #expect(renderer.registry.handle(for: NodeId(4))?.view as? NSTextField === statusField)
        for (handle, id) in zip(summaryHandles, Self.summaryNodeIDs) {
            #expect(renderer.registry.handle(for: NodeId(id)) === handle, "summary node \(id) was rebuilt")
        }
        #expect(window.windowNumber == windowNumber)
        #expect(window.isVisible)
        #expect(table.tableColumns.map(\.title) == ["PID", "Name", "Resident", "CPU (100% = 1 CPU)"])
        #expect(tableHandle.actionTrampoline == nil)
        try await Self.capture(window: window, name: "sequence-settled")
        let elapsed = stamps.isEmpty ? Duration.zero : stamps[stamps.count - 1]
        print("""
        PX-004/PX-007 native SSH evidence: interval=\(intervalMilliseconds)ms window \(windowNumber) retained; \
        observed \(trace.count) published states in \(elapsed); revisions per five-tick cycle=\(cycle); \
        unchanged row ItemId=\(unchanged); \(summaryHandles.count) summary handles retained; \
        failed freshness=\(failedFreshness); recovered freshness=\(recoveredFreshness); \
        states=\(trace.map { "\($0.revision):\($0.rows.map { $0[0] }.joined(separator: ","))" })
        """)
        await controller.stop()
    }

    /// PX-008, the R0 gate: once a client is synchronized, a frozen fake source sends it no app UI
    /// mutation (§4 invariant 12, §12.2). This is the real binary at its shortest interval, behind
    /// the real bridge and sshd, received by the unchanged generic client through a pass-through
    /// transport that records every framed message (`RecordingTransport`).
    ///
    /// An app UI mutation is a `Transaction`, the §19.2 UI class and the only message that changes
    /// the replica (§12.1). Control traffic — WELCOME, RESUME_OK, RESYNC_REQUIRED, handshake
    /// refusals and EVENT_ACKs — is session management, which invariant 12 allows an idle UI; it
    /// is counted, not excluded by assumption. The window ends with a canary, the `--smoke-fixture`
    /// retitle: transactions arrive in commit order, so the first one after synchronization must
    /// be the canary, and its arrival proves the client was receiving throughout. The loop's own
    /// ticks are counted by `apps/srtop/tests/release_gate_test.rs`, which a client cannot see.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func aFrozenFakeSourceSendsNoAppUIMutationAfterSynchronization() async throws {
        let harness = try await Self.launch(arguments: [
            "--smoke-fixture", "--fake-source", "--refresh-interval-ms", "50",
        ])
        defer { Self.shutDown(harness) }

        _ = NSApplication.shared
        let transport = RecordingTransport(inner: SSHTransport(configuration: harness.configuration))
        let (controller, applier, renderer) = Self.genericClient(transport)
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "synchronized fake shell over SSH") {
            applier.lastAppliedRevision == Revision(1) && renderer.registry.handle(for: NodeId(5)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        SurfacePresentation.forHostApplication().present(window)
        let synchronized = transport.frames.count

        // The observation window is the subject of the test, not a synchronization: three
        // wall-clock seconds of the server's 50 ms loop, in which nothing but control traffic may
        // arrive. However slow the machine, a longer or shorter window cannot fail a correct server.
        let clock = ContinuousClock()
        let opened = clock.now
        try await Task.sleep(for: .seconds(3))
        let observed = opened.duration(to: clock.now)
        #expect(applier.lastAppliedRevision == Revision(1), "a frozen source advanced the client's revision")
        let screenshot = try WindowEvidence.capture(window, name: "r0-fake-source")

        #expect(kill(harness.server.processIdentifier, SIGUSR1) == 0)
        try await AsyncTestSupport.eventually(timeout: .seconds(5), description: "the canary over SSH") {
            applier.lastAppliedRevision == Revision(2) && window.title == "Process Explorer — title fixture"
        }
        let frames = transport.frames
        let afterSync = Array(frames[synchronized...])
        let canary = try #require(afterSync.firstIndex { $0.logicalClass == "ui" }, "the canary never arrived")
        let beforeCanary = afterSync[..<canary]
        #expect(beforeCanary.allSatisfy { $0.logicalClass == "control" },
                "application traffic after synchronization: \(beforeCanary.map(\.message))")
        #expect(afterSync[canary].message == "Transaction")
        #expect(afterSync[canary].baseRevision == 1 && afterSync[canary].newRevision == 2,
                "the first transaction after synchronization is the canary")
        #expect(afterSync[canary].operations == ["SET_PROPERTY", "SET_PROPERTY"])
        #expect(afterSync[(canary + 1)...].isEmpty, "traffic after the canary: \(afterSync[(canary + 1)...].map(\.message))")
        // The trace accounts for every byte the client received.
        let counts = transport.byteCounts
        #expect(frames.reduce(0) { $0 + $1.bytes } == counts.received - counts.pending)

        try WindowEvidence.write(transport.traceText(
            title: "srtop --fake-source --refresh-interval-ms 50 --smoke-fixture over a real localhost "
                + "sshd and srui-ssh-bridge; frozen window of \(observed) after synchronization, then the "
                + "SIGUSR1 canary"
        ), name: "r0-fake-source-trace")
        print("""
        PX-008 frozen-source native evidence: window \(window.windowNumber); synchronized after \
        \(synchronized) frames (\(frames[..<synchronized].map { "\($0.message) \($0.bytes) B" })); \
        observed \(observed) at a 50 ms interval; control frames in the window=\(beforeCanary.count); \
        app UI mutations in the window=0; canary=Transaction 1->2 \(afterSync[canary].operations) \
        (\(afterSync[canary].bytes) B); screenshot=\(screenshot ?? "not requested")
        """)
        await controller.stop()
    }

    /// PX-008, the R0 gate: the same generic client runs the counter example and the explorer —
    /// one client construction, the one `RendererDemoApp` performs for every live session, with no
    /// application-specific setting, over the same real SSH subsystem path.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func theSameGenericClientRunsTheCounterAndTheExplorer() async throws {
        _ = NSApplication.shared
        // The counter: one activation, answered by the server with a new count.
        let counter = try await Self.launch(
            binary: "examples/counter/target/debug/counter", arguments: [])
        do {
            defer { Self.shutDown(counter) }
            let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: counter.configuration))
            try await controller.start()
            // Events may be sent once the catch-up snapshot has been applied and the session has
            // reopened dispatch (§15, §18); a rendered count alone does not mean that yet.
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the counter over SSH") {
                applier.lastAppliedRevision == Revision(1)
                    && controller.isEventDispatchEnabled
                    && (renderer.registry.handle(for: NodeId(2))?.view as? NSTextField)?.stringValue == "Count: 0"
            }
            let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
            defer { window.orderOut(nil); window.close() }
            _ = try await controller.sendActivate(nodeId: NodeId(4))
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the counter's answer over SSH") {
                applier.lastAppliedRevision == Revision(2)
                    && (renderer.registry.handle(for: NodeId(2))?.view as? NSTextField)?.stringValue == "Count: 1"
            }
            print("PX-008 same-client evidence: counter window \(window.windowNumber) '\(window.title)' showed Count: 0 -> Count: 1 at revision 2")
            await controller.stop()
        }
        // The explorer: the fixed fake snapshot in the native table.
        let explorer = try await Self.launch(arguments: ["--fake-source"])
        defer { Self.shutDown(explorer) }
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: explorer.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH") {
            applier.lastAppliedRevision == Revision(1) && renderer.registry.handle(for: NodeId(5)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let scroll = try #require(renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        #expect(window.title == "Process Explorer")
        #expect(Self.nativeRows(table) == [
            ["4101", "worker", "1.1 MiB", "250.0%"],
            ["4102", "worker", "0 B", "0.0%"],
            ["Unavailable", "helper", "Denied", "Warming up"],
        ])
        print("PX-008 same-client evidence: explorer window \(window.windowNumber) '\(window.title)' rows=\(Self.nativeRows(table))")
        await controller.stop()
    }

    /// The client `RendererDemoApp --ssh/--socket` builds for every live session: nothing in it
    /// knows which application is on the other end.
    @MainActor
    fileprivate static func genericClient(
        _ transport: any Transport
    ) -> (SessionController, TransactionApplier, AppKitRenderer) {
        let renderer = AppKitRenderer()
        let applier = TransactionApplier()
        let controller = SessionController(transport: transport, applier: applier,
                                           outbox: EventOutbox(), renderer: renderer)
        controller.attachRenderer(renderer)
        return (controller, applier, renderer)
    }

    /// What a client actually holds and shows at one moment.
    fileprivate struct Observation: Equatable, Sendable {
        let revision: UInt64
        let status: String
        let rows: [[String]]
        let itemIDs: [UInt64]
        /// The native summary lines, the freshness line last (PX-007).
        let summary: [String]
    }

    // PX-007: the summary's nodes — its column, three rows of a bar and a line, and four more
    // lines — and the seven lines in the order they appear, the freshness line last.
    fileprivate static let summaryNodeIDs: [UInt64] = Array(6...19)
    fileprivate static let summaryLineIDs: [UInt64] = [9, 12, 15, 16, 17, 18, 19]
    fileprivate static let summaryBarIDs: [UInt64] = [8, 11, 14]

    private static let fakeSummary = [
        "Overall CPU (100% = all 8 logical CPUs): 31.2%",
        "Memory: 4.0 GiB used of 16.0 GiB (25.0% of total)",
        "Swap: none configured",
        "Load average (1, 5, 15 min): 0.52, 0.58, 0.59",
        "Uptime: 3 days, 4 h 05 min",
        "Processes visible to this reader: 3 listed · complete scan · no srtop filter",
        "Last successful sample: 2027-01-15 08:00:00 UTC (server clock) · source: fake-processes-v1",
    ]
    private static let unsampledSummary = [
        "Overall CPU (100% = all logical CPUs): Not sampled",
        "Memory: Not sampled",
        "Swap: Not sampled",
        "Load average (1, 5, 15 min): Not sampled",
        "Uptime: Not sampled",
        "Processes visible to this reader: Not sampled",
        "No sample: process collection not started",
    ]
    /// The script's figures, the same on every step; only its freshness line moves.
    private static let scriptedFigures = [
        "Overall CPU (100% = all 4 logical CPUs): 30.0%",
        "Memory: 2.0 GiB used of 8.0 GiB (25.0% of total)",
        "Swap: 256.0 MiB used of 2.0 GiB (12.5% of total)",
        "Load average (1, 5, 15 min): 1.25, 1.00, 0.75",
        "Uptime: 1 day, 2 h 30 min",
        "Processes visible to this reader: 3 listed · complete scan · no srtop filter",
    ]

    @MainActor
    fileprivate static func summaryHandles(_ renderer: AppKitRenderer) throws -> [RenderHandle] {
        try summaryNodeIDs.map { id in try #require(renderer.registry.handle(for: NodeId(id)), "summary node \(id)") }
    }

    @MainActor
    fileprivate static func summaryLines(_ renderer: AppKitRenderer) throws -> [NSTextField] {
        try summaryLineIDs.map { id in
            try #require(renderer.registry.handle(for: NodeId(id))?.view as? NSTextField, "summary line \(id)")
        }
    }

    @MainActor
    private static func summaryBars(_ renderer: AppKitRenderer) throws -> [NSProgressIndicator] {
        try summaryBarIDs.map { id in
            try #require(renderer.registry.handle(for: NodeId(id))?.view as? NSProgressIndicator, "summary bar \(id)")
        }
    }

    /// The seconds of the scripted sample time a freshness line names (`2027-01-15 08:00:SS`).
    private static func sampleSecond(_ line: String) -> Int? {
        guard let date = line.range(of: "2027-01-15 08:") else { return nil }
        let clock = line[date.upperBound...].prefix(5)
        let parts = clock.split(separator: ":")
        guard parts.count == 2, let minutes = Int(parts[0]), let seconds = Int(parts[1]) else { return nil }
        return minutes * 60 + seconds
    }

    /// The first state of each run of the script's initial rows that follows another state:
    /// one per cycle, at its step 0.
    private static func cycleStarts(_ trace: [Observation]) -> [Int] {
        trace.indices.filter { $0 > 0 && trace[$0].rows == initialRows && trace[$0 - 1].rows != initialRows }
    }

    // PX-005: every scripted row carries its resident cell, fixed per process so
    // that a repeated snapshot's rows stay byte-identical and publish nothing.
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

    /// Whether the recorded states cover every state the PX-004 and PX-007 assertions read.
    private static func coversTwoScriptedCycles(_ trace: [Observation]) -> Bool {
        guard cycleStarts(trace).count >= 2,
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
        modelText(applier, NodeId(4))
    }

    /// The summary lines the client holds, in display order (PX-007).
    @MainActor
    fileprivate static func modelSummary(_ applier: TransactionApplier) -> [String] {
        summaryLineIDs.map { modelText(applier, NodeId($0)) }
    }

    @MainActor
    private static func modelText(_ applier: TransactionApplier, _ id: NodeId) -> String {
        guard case .string(let text)? = applier.store.getNode(id)?.getProperty(.text) else {
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
    private static func launch(
        binary: String = "apps/srtop/target/debug/srtop", arguments: [String]
    ) async throws -> Harness {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let appBinary = root.appendingPathComponent(binary)
        let bridgeBinary = root.appendingPathComponent("server-rust/target/debug/srui-ssh-bridge")
        try #require(FileManager.default.isExecutableFile(atPath: appBinary.path),
                     "Run bash apps/srtop/test.sh to build the required server \(binary)")
        try #require(FileManager.default.isExecutableFile(atPath: bridgeBinary.path),
                     "The real SSH bridge is required; this test must not soft-skip")
        let temp = try TestFixtureDirectory.make(prefix: "px001")
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
            TestFixtureDirectory.release(temp)
            throw error
        }
    }

    private static func shutDown(_ harness: Harness) {
        if harness.server.isRunning { kill(harness.server.processIdentifier, SIGINT) }
        harness.server.waitUntilExit()
        SSHTestSupport.terminate(harness.sshd)
        // Last, so the sshd and the app server that hold this directory's socket and host key are
        // already gone (see `TestFixtureDirectory`).
        TestFixtureDirectory.release(harness.temp)
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
    private let summaryLines: [NSTextField]
    private let clock = ContinuousClock()
    /// When recording began; `stamps` are measured from here.
    let start: ContinuousClock.Instant

    init(applier: TransactionApplier, window: NSWindow, statusField: NSTextField, table: NSTableView,
         summaryLines: [NSTextField]) {
        self.applier = applier
        self.window = window
        self.statusField = statusField
        self.table = table
        self.summaryLines = summaryLines
        self.start = clock.now
    }

    func record() {
        window.contentView?.layoutSubtreeIfNeeded()
        let observation = ProcessExplorerShellTests.Observation(
            revision: applier.lastAppliedRevision.value,
            status: statusField.stringValue,
            rows: ProcessExplorerShellTests.nativeRows(table),
            itemIDs: ProcessExplorerShellTests.modelItemIDs(applier),
            summary: summaryLines.map(\.stringValue)
        )
        let modelRows = ProcessExplorerShellTests.modelRows(applier)
        let modelStatus = ProcessExplorerShellTests.modelStatus(applier)
        let modelSummary = ProcessExplorerShellTests.modelSummary(applier)
        if observation.rows != modelRows || observation.status != modelStatus
            || observation.summary != modelSummary {
            divergences.append(
                "rev \(observation.revision): native \(observation.rows)/\(observation.status)/\(observation.summary)"
                    + " vs model \(modelRows)/\(modelStatus)/\(modelSummary)"
            )
        }
        guard states.last != observation else { return }
        states.append(observation)
        stamps.append(start.duration(to: clock.now))
    }
}

/// PX-008's op/byte trace: a pass-through transport that also splits a copy of every chunk the
/// client receives into the framed SRUI messages it carries (§16), recording each one's size on
/// the wire, its §19.2 logical class, its revisions and its operations. The client receives the
/// same chunks unchanged and every acknowledgement is forwarded, so flow control stays the inner
/// transport's own (§14, §26). Visual evidence only: a trace is not a conformance oracle (§32).
fileprivate final class RecordingTransport: Transport, @unchecked Sendable {
    struct Frame: Sendable {
        /// Since this transport was created.
        let offset: Duration
        /// The varint length prefix plus the payload, as received.
        let bytes: Int
        let message: String
        /// The §19.2 class: control, ui, resource or terminal.
        let logicalClass: String
        let baseRevision: UInt64?
        let newRevision: UInt64?
        let operations: [String]
    }

    private let inner: any Transport
    private let lock = NSLock()
    private let clock = ContinuousClock()
    private let start: ContinuousClock.Instant
    private var pending = Data()
    private var recorded: [Frame] = []
    private var received = 0

    init(inner: any Transport) {
        self.inner = inner
        start = clock.now
    }

    var frames: [Frame] {
        lock.lock()
        defer { lock.unlock() }
        return recorded
    }

    /// Every byte received, and how many of them do not complete a frame yet.
    var byteCounts: (received: Int, pending: Int) {
        lock.lock()
        defer { lock.unlock() }
        return (received, pending.count)
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        try await inner.send(data: data, logicalClass: logicalClass)
    }

    func close() async {
        await inner.close()
    }

    func acknowledgeReceived(byteCount: Int) async {
        await inner.acknowledgeReceived(byteCount: byteCount)
    }

    func receiveStream() -> AsyncThrowingStream<Data, Error> {
        let inner = self.inner
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await chunk in inner.receiveStream() {
                        self.observe(chunk)
                        continuation.yield(chunk)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func observe(_ chunk: Data) {
        let offset = start.duration(to: clock.now)
        lock.lock()
        defer { lock.unlock() }
        received += chunk.count
        pending.append(chunk)
        while let prefix = Self.lengthPrefix(pending), pending.count >= prefix.header + prefix.length {
            let base = pending.startIndex
            let payload = pending.subdata(in: (base + prefix.header)..<(base + prefix.header + prefix.length))
            recorded.append(Self.frame(payload, bytes: prefix.header + prefix.length, offset: offset))
            pending = pending.subdata(in: (base + prefix.header + prefix.length)..<pending.endIndex)
        }
    }

    private static func lengthPrefix(_ data: Data) -> (length: Int, header: Int)? {
        var value = 0
        for (index, byte) in data.prefix(10).enumerated() {
            value |= Int(byte & 0x7f) << (7 * index)
            if byte & 0x80 == 0 {
                return (value, index + 1)
            }
        }
        return nil
    }

    private static func frame(_ payload: Data, bytes: Int, offset: Duration) -> Frame {
        func make(_ message: String, _ logicalClass: String,
                  _ transaction: SRUITransaction? = nil) -> Frame {
            Frame(offset: offset, bytes: bytes, message: message, logicalClass: logicalClass,
                  baseRevision: transaction?.baseRevision, newRevision: transaction?.newRevision,
                  operations: transaction?.operations.map(operationName) ?? [])
        }
        guard let message = try? SRUIMessage(serializedBytes: payload) else {
            return make("undecodable", "undecodable")
        }
        switch message.msg {
        case .serverWelcome: return make("ServerWelcome", "control")
        case .serverResumeOk: return make("ServerResumeOk", "control")
        case .serverResyncRequired: return make("ServerResyncRequired", "control")
        case .serverHandshakeRefused: return make("ServerHandshakeRefused", "control")
        case .serverEventAck: return make("ServerEventAck", "control")
        case .transaction(let transaction): return make("Transaction", "ui", transaction)
        case .resourceMetadata: return make("ResourceMetadata", "resource")
        case .resourceChunk: return make("ResourceChunk", "resource")
        case .terminalData: return make("TerminalData", "terminal")
        case .terminalResyncRequired: return make("TerminalResyncRequired", "terminal")
        default: return make("unexpected", "unexpected")
        }
    }

    private static func operationName(_ operation: SRUIOperation) -> String {
        switch operation.op {
        case .createNode: "CREATE_NODE"
        case .deleteNode: "DELETE_NODE"
        case .setProperty: "SET_PROPERTY"
        case .clearProperty_p: "CLEAR_PROPERTY"
        case .commit: "COMMIT"
        case .createModel: "CREATE_MODEL"
        case .modelInsert: "MODEL_INSERT"
        case .modelDelete: "MODEL_DELETE"
        case .modelUpdate: "MODEL_UPDATE"
        case .modelResetRange: "MODEL_RESET_RANGE"
        case .moveNode: "MOVE_NODE"
        case .reorderChildren: "REORDER_CHILDREN"
        case .batchPropertySet: "BATCH_PROPERTY_SET"
        case nil: "EMPTY"
        }
    }

    /// The trace as a small table for a person to read: one line per received frame, then totals.
    func traceText(title: String) -> String {
        let frames = self.frames
        func pad(_ text: String, _ width: Int, left: Bool = false) -> String {
            let fill = String(repeating: " ", count: max(0, width - text.count))
            return left ? text + fill : fill + text
        }
        func operations(_ names: [String]) -> String {
            var order: [String] = []
            var counts: [String: Int] = [:]
            for name in names {
                if counts[name] == nil { order.append(name) }
                counts[name, default: 0] += 1
            }
            return order.isEmpty ? "-" : order.map { "\($0)x\(counts[$0]!)" }.joined(separator: " ")
        }
        var lines = [
            "# PX-008 op/byte trace: \(title)",
            "# Every framed SRUI message the unchanged generic client received, split from a copy of its",
            "# byte stream by a pass-through test transport. Visual evidence only, not semantic",
            "# conformance (D1 §32). Bytes are frame sizes on the wire: varint prefix plus payload.",
            "#  t(ms)    bytes  class    message                revisions  operations",
        ]
        for frame in frames {
            let milliseconds = frame.offset.components.seconds * 1000
                + frame.offset.components.attoseconds / 1_000_000_000_000_000
            let revisions = frame.baseRevision.map { "\($0)->\(frame.newRevision ?? 0)" } ?? "-"
            lines.append(pad(String(milliseconds), 8) + " " + pad(String(frame.bytes), 8) + "  "
                + pad(frame.logicalClass, 7, left: true) + "  " + pad(frame.message, 22, left: true) + " "
                + pad(revisions, 10, left: true) + " " + operations(frame.operations))
        }
        var totals: [String] = []
        for logicalClass in ["control", "ui", "resource", "terminal", "unexpected", "undecodable"] {
            let members = frames.filter { $0.logicalClass == logicalClass }
            if !members.isEmpty {
                totals.append("\(logicalClass) \(members.count) frames / \(members.reduce(0) { $0 + $1.bytes }) B")
            }
        }
        lines.append("# totals: \(frames.count) frames / \(frames.reduce(0) { $0 + $1.bytes }) B; "
            + totals.joined(separator: "; "))
        return lines.joined(separator: "\n") + "\n"
    }
}

/// PX-008's visual evidence, written only when `PX008_EVIDENCE_DIR` names a directory: the client
/// window alone — never the desktop or another window — and the op/byte trace. Screenshots are
/// not conformance criteria (D1 §32).
@MainActor
fileprivate enum WindowEvidence {
    static var directory: URL? {
        ProcessInfo.processInfo.environment["PX008_EVIDENCE_DIR"].map { URL(fileURLWithPath: $0) }
    }

    static func write(_ text: String, name: String) throws {
        guard let directory else { return }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try text.write(to: directory.appendingPathComponent("\(name).txt"), atomically: true, encoding: .utf8)
    }

    /// Renders `window` alone — title bar, background and content, at one pixel per point —
    /// through AppKit's own drawing of its frame view, and returns how the image was made.
    ///
    /// Not `screencapture -l`: a `swift test` host never runs an `NSApplication` event loop (its
    /// activation policy is `.prohibited`), so its windows are never flushed to the window server,
    /// and a window-server capture of one is a blank white rectangle even with Screen Recording
    /// permission. Measured on this machine while writing PX-008; the frame view is what AppKit
    /// drew. PX-001's content-view capture left the labels unreadable on a transparent backdrop;
    /// the frame view draws the window background behind them.
    static func capture(_ window: NSWindow, name: String) throws -> String? {
        guard let directory else { return nil }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("\(name).png")
        window.contentView?.layoutSubtreeIfNeeded()
        window.displayIfNeeded()
        guard let frameView = window.contentView?.superview,
              let bitmap = NSBitmapImageRep(
                  bitmapDataPlanes: nil, pixelsWide: Int(frameView.bounds.width),
                  pixelsHigh: Int(frameView.bounds.height), bitsPerSample: 8, samplesPerPixel: 4,
                  hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
        else {
            return "no image: the window has no frame view to render"
        }
        bitmap.size = frameView.bounds.size
        frameView.cacheDisplay(in: frameView.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else {
            return "no image: the rendering could not be encoded"
        }
        try png.write(to: url)
        return "AppKit rendering of window \(window.windowNumber)'s frame view only "
            + "(\(bitmap.pixelsWide)x\(bitmap.pixelsHigh) px, \(png.count) B)"
    }
}

/// Runs a fixed argument vector to completion and returns its status and combined output, without
/// `waitUntilExit()` (which would service the main run loop the main actor runs on).
fileprivate func runToCompletion(_ executable: String, _ arguments: [String]) async throws -> (status: Int32, output: String) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    let pipe = Pipe()
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = pipe
    process.standardError = pipe
    let output = pipe.fileHandleForReading
    return try await withCheckedThrowingContinuation { continuation in
        process.terminationHandler = { finished in
            let text = String(decoding: output.readDataToEndOfFile(), as: UTF8.self)
            continuation.resume(returning: (finished.terminationStatus, text.trimmingCharacters(in: .whitespacesAndNewlines)))
        }
        do {
            try process.run()
        } catch {
            process.terminationHandler = nil
            continuation.resume(throwing: error)
        }
    }
}

/// Where the devbox is, as `apps/srtop/devbox/r0-scenario.sh` exports it.
fileprivate struct DevboxScenario: Sendable {
    let configuration: SSHConfiguration
    let docker: String
    let container: String

    static func fromEnvironment() -> DevboxScenario? {
        let environment = ProcessInfo.processInfo.environment
        guard let port = environment["SRTOP_R0_DEVBOX_PORT"].flatMap({ UInt16($0) }),
              let key = environment["SRTOP_R0_DEVBOX_KEY"],
              let knownHosts = environment["SRTOP_R0_DEVBOX_KNOWN_HOSTS"],
              let docker = environment["SRTOP_R0_DOCKER"],
              let container = environment["SRTOP_R0_DEVBOX_CONTAINER"] else {
            return nil
        }
        return DevboxScenario(
            configuration: SSHConfiguration(
                host: "127.0.0.1", port: port, user: "srui", subsystem: "srui", identityFile: key,
                knownHostsFile: knownHosts, strictHostKeyChecking: .yes, batchMode: true, connectTimeout: 10),
            docker: docker, container: container)
    }
}

/// PX-008, the R0 end-to-end scenario on a live Linux `/proc`: the devbox's `srtop --live-source`
/// behind the real `srui-ssh-bridge` subsystem, the unchanged generic client, a worker the box runs
/// and ends on its own. Opt-in, because it needs Docker: `apps/srtop/devbox/r0-scenario.sh` starts
/// the box and runs this suite with the box's coordinates in the environment. Without them the
/// suite is reported as skipped, never as passed.
@Suite(.serialized, .enabled(if: DevboxScenario.fromEnvironment() != nil,
                             "needs the srtop devbox: run apps/srtop/devbox/r0-scenario.sh"))
struct ProcessExplorerDevboxScenarioTests {
    /// `bash` under its own name, so the kernel's `comm` — the name srtop publishes — is this one
    /// from the moment it runs.
    static let workerPath = "/home/srui/r0-worker"
    static let workerName = "r0-worker"
    /// A busy loop of shell builtins, no child process, that ends by itself after eight seconds.
    static let workerScript = "while [ $SECONDS -lt 8 ]; do :; done"
    static let liveStatus = "Read-only · Live process snapshot"

    @Test(.timeLimit(.minutes(2)))
    @MainActor
    func aWorkerAppearsIsSampledAndDisappearsFromAnEmptyWindow() async throws {
        let box = try #require(DevboxScenario.fromEnvironment())
        let prepare = try await runToCompletion(
            box.docker, ["exec", "-u", "srui", box.container, "ln", "-sf", "/bin/bash", Self.workerPath])
        try #require(prepare.status == 0, "could not prepare the worker: \(prepare.output)")

        _ = NSApplication.shared
        let transport = RecordingTransport(inner: SSHTransport(configuration: box.configuration))
        let (controller, applier, renderer) = ProcessExplorerShellTests.genericClient(transport)
        // An empty window: the client holds and shows nothing before the session does.
        #expect(renderer.registry.surfaceHandles.isEmpty)
        #expect(applier.lastAppliedRevision.value == 0)
        let clock = ContinuousClock()
        let started = clock.now
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(20), description: "the live process table over SSH") {
            applier.lastAppliedRevision.value >= 1 && renderer.registry.handle(for: NodeId(5)) != nil
        }
        let surfaceHandle = try #require(renderer.registry.handle(for: NodeId(1)))
        let window = try #require(surfaceHandle.window)
        defer { window.orderOut(nil); window.close() }
        let statusField = try #require(renderer.registry.handle(for: NodeId(4))?.view as? NSTextField)
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        let summaryHandles = try ProcessExplorerShellTests.summaryHandles(renderer)
        let summaryLines = try ProcessExplorerShellTests.summaryLines(renderer)
        SurfacePresentation.forHostApplication().present(window)
        window.contentView?.layoutSubtreeIfNeeded()
        let windowNumber = window.windowNumber

        let sessionFailure = ManagedAtomic<String?>(nil)
        controller.onFailure = { failure in sessionFailure.store(String(describing: failure)) }
        let recorder = PublishedStateRecorder(applier: applier, window: window, statusField: statusField,
                                              table: table, summaryLines: summaryLines)
        controller.rendererDidRenderInterceptorForTesting = { [recorder] in
            await recorder.record()
        }
        defer { controller.rendererDidRenderInterceptorForTesting = nil }
        recorder.record()
        let initial = try #require(recorder.states.first)
        #expect(initial.status == Self.liveStatus)
        #expect(!initial.rows.contains { $0[1] == Self.workerName })
        #expect(table.tableColumns.map(\.title) == ["PID", "Name", "Resident", "CPU (100% = 1 CPU)"])

        // Worker creation, inside the box, as the box's own unprivileged user.
        let spawned = clock.now
        let spawn = try await runToCompletion(
            box.docker, ["exec", "-d", "-u", "srui", box.container, Self.workerPath, "-c", Self.workerScript])
        try #require(spawn.status == 0, "could not start the worker: \(spawn.output)")

        // It appears: a new row, first published as warming up, with a resident size in bytes.
        let appearedIndex = try await Self.waitForState(recorder, failure: sessionFailure,
                                                        description: "the worker's row") { state in
            state.rows.contains { $0[1] == Self.workerName }
        }
        let appeared = recorder.states[appearedIndex]
        let appearedAfter = recorder.stamps[appearedIndex]
        try #require(appeared.itemIDs.count == appeared.rows.count, "native rows and model items disagree")
        let workerRow = try #require(appeared.rows.firstIndex { $0[1] == Self.workerName })
        let workerItem = appeared.itemIDs[workerRow]
        let workerPID = appeared.rows[workerRow][0]
        #expect(UInt32(workerPID) != nil, "the worker's PID is known: \(workerPID)")
        #expect(appeared.rows[workerRow][3] == "Warming up", "a process seen for the first time warms up")
        #expect(Self.isByteCount(appeared.rows[workerRow][2]), "resident memory: \(appeared.rows[workerRow][2])")

        // It is sampled: a later scan measures its CPU over an interval.
        let sampledIndex = try await Self.waitForState(recorder, failure: sessionFailure, after: appearedIndex,
                                                       description: "the worker's measured CPU") { state in
            zip(state.itemIDs, state.rows).contains { $0 == workerItem && Self.isPercentage($1[3]) }
        }
        let sampled = recorder.states[sampledIndex]
        try #require(sampled.itemIDs.count == sampled.rows.count, "native rows and model items disagree")
        let sampledRow = try #require(sampled.itemIDs.firstIndex(of: workerItem))
        #expect(sampled.rows[sampledRow][0] == workerPID && sampled.rows[sampledRow][1] == Self.workerName)
        #expect(Self.isByteCount(sampled.rows[sampledRow][2]))
        // Window geometry is local presentation (§4 invariant 3): make room for every row before
        // the picture, and let the renderer's own layout fill it.
        window.setContentSize(NSSize(width: max(window.frame.width, 640), height: 600))
        let screenshot = try WindowEvidence.capture(window, name: "r0-devbox-live")

        // It exits by itself, and its row is deleted rather than kept or renamed.
        let goneIndex = try await Self.waitForState(recorder, failure: sessionFailure, timeout: .seconds(30),
                                                    after: sampledIndex, description: "the worker's exit") { state in
            !state.itemIDs.contains(workerItem)
        }
        let gone = recorder.states[goneIndex]
        let goneAfter = recorder.stamps[goneIndex]
        #expect(goneIndex > sampledIndex)
        #expect(!gone.rows.contains { $0[1] == Self.workerName })
        controller.rendererDidRenderInterceptorForTesting = nil
        let states = recorder.states

        // Across the whole run: every scan complete, the table and the summary in step, the
        // native controls showing what the client holds and built once, and srtop's own row one
        // identity throughout.
        #expect(recorder.divergences.isEmpty, "native controls diverged from the client's model")
        for state in states {
            #expect(state.status == Self.liveStatus, "rev \(state.revision): \(state.status)")
            #expect(state.summary[5] == "Processes visible to this reader: \(state.rows.count) listed · complete scan · no srtop filter",
                    "rev \(state.revision): \(state.summary[5])")
            #expect(state.summary[6].hasPrefix("Last successful sample: ")
                        && state.summary[6].hasSuffix(" UTC (server clock) · source: procfs:/proc"), "\(state.summary[6])")
        }
        #expect(states[appearedIndex..<goneIndex].allSatisfy { $0.itemIDs.contains(workerItem) },
                "the worker's row left and came back before the worker ended")
        #expect(states[goneIndex...].allSatisfy { !$0.itemIDs.contains(workerItem) }, "the worker's identity came back")
        let srtopItems = Set(states.compactMap { state in zip(state.rows, state.itemIDs).first { $0.0[1] == "srtop" }?.1 })
        #expect(srtopItems.count == 1, "srtop's own row changed identity: \(srtopItems)")
        #expect(states.contains { Self.isMeasuredOverallCPU($0.summary[0]) }, "overall CPU was never measured")
        #expect(Set(states.map { $0.summary[6] }).count > 1, "the freshness line never advanced")
        #expect(renderer.registry.handle(for: NodeId(1)) === surfaceHandle)
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(renderer.registry.handle(for: NodeId(4))?.view as? NSTextField === statusField)
        for (handle, id) in zip(summaryHandles, ProcessExplorerShellTests.summaryNodeIDs) {
            #expect(renderer.registry.handle(for: NodeId(id)) === handle, "summary node \(id) was rebuilt")
        }
        #expect(window.windowNumber == windowNumber && window.isVisible)
        #expect(sessionFailure.load() == nil)

        let frames = transport.frames
        let counts = transport.byteCounts
        #expect(frames.reduce(0) { $0 + $1.bytes } == counts.received - counts.pending)
        try WindowEvidence.write(transport.traceText(
            title: "devbox srtop --live-source --refresh-interval-ms 1000 behind srui-ssh-bridge; "
                + "worker \(Self.workerName) started \(started.duration(to: spawned)) after connecting"
        ), name: "r0-devbox-live-trace")
        print("""
        PX-008 devbox evidence: window \(windowNumber) retained; \(states.count) published states; \
        initial rows=\(initial.rows.count) processes=\(initial.summary[5]); worker pid=\(workerPID) item=\(workerItem); \
        appeared \(spawned.duration(to: recorder.start + appearedAfter)) after the spawn: \(appeared.rows[workerRow]) \
        processes=\(appeared.summary[5]); sampled: \(sampled.rows[sampledRow]); \
        gone \(spawned.duration(to: recorder.start + goneAfter)) after the spawn, rows=\(gone.rows.count) processes=\(gone.summary[5]); \
        cpu=\(sampled.summary[0]); memory=\(sampled.summary[1]); swap=\(sampled.summary[2]); \
        load=\(sampled.summary[3]); uptime=\(sampled.summary[4]); first freshness=\(initial.summary[6]); \
        last freshness=\(gone.summary[6]); screenshot=\(screenshot ?? "not requested"); \
        trace=\(frames.count) frames/\(counts.received) B
        """)
        await controller.stop()
    }

    /// The index of the first recorded state that satisfies `condition`, waiting for the renderer's
    /// own per-transaction record to produce it; a failed session names itself.
    @MainActor
    private static func waitForState(
        _ recorder: PublishedStateRecorder, failure: ManagedAtomic<String?>, timeout: Duration = .seconds(20),
        after earlier: Int = -1, description: String,
        _ condition: (ProcessExplorerShellTests.Observation) -> Bool
    ) async throws -> Int {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while true {
            let states = recorder.states
            if let index = states.indices.first(where: { $0 > earlier && condition(states[$0]) }) {
                return index
            }
            guard clock.now < deadline, failure.load() == nil else {
                throw AsyncTestTimeout(description: "no state showed \(description); session failure: "
                    + "\(failure.load() ?? "none"); last: \(recorder.states.last.map { "\($0.revision) \($0.rows)" } ?? "none")")
            }
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    /// `0 B`, `512 B`, `3.4 MiB`: the IEC text srtop publishes for a known resident size.
    private static func isByteCount(_ text: String) -> Bool {
        text.range(of: #"^[0-9]+(\.[0-9])? (B|KiB|MiB|GiB|TiB|PiB|EiB)$"#, options: .regularExpression) != nil
    }

    /// `0.0%`, `99.8%`: a CPU share measured over an interval.
    private static func isPercentage(_ text: String) -> Bool {
        text.range(of: #"^[0-9]+\.[0-9]%$"#, options: .regularExpression) != nil
    }

    private static func isMeasuredOverallCPU(_ text: String) -> Bool {
        text.range(of: #"^Overall CPU \(100% = all [0-9]+ logical CPUs\): [0-9]+\.[0-9]%$"#,
                   options: .regularExpression) != nil
    }
}
