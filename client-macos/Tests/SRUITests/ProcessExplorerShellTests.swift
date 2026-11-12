// PX-001/PX-002/PX-004/PX-005/PX-006/PX-007/PX-008/PX-009/PX-012/PX-013/PX-014/PX-015/PX-016/PX-017: real non-PTY SSH launch through the unchanged generic client (§§4, 6.2, 7.6, 8, 12, 19.2, 22, 29, 31).
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

    /// PX-009: the sort buttons are standard native buttons; a click is a standard `ACTIVATE`
    /// (§7.6), the server reorders its own rows, and the generic client shows the new order in
    /// the same native table with the selection still on the same process, because the rows keep
    /// their item IDs (§6.2, §8) and the client anchors its selection to them.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func sortButtonsReorderTheNativeTableAndTheSelectionStaysOnItsProcess() async throws {
        let harness = try await Self.launch(arguments: ["--fake-source"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        // Clicks may be sent once the catch-up snapshot is applied and dispatch has reopened.
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH") {
            applier.lastAppliedRevision == Revision(1)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        let buttonHandles = try Self.sortButtonIDs.map { id in
            try #require(renderer.registry.handle(for: NodeId(id)), "sort button \(id)")
        }
        let buttons = try buttonHandles.map { handle in try #require(handle.view as? NSButton) }
        // Nothing is sorted yet: the source's order, and no button claims an order.
        #expect(buttons.map(\.title) == ["Sort by PID", "Sort by Name", "Sort by Resident", "Sort by CPU"])
        #expect(buttons.allSatisfy { $0.isEnabled })
        #expect(Self.nativeRows(table) == Self.fakeRows)
        let identities = Self.modelItemIDs(applier)
        #expect(identities.count == 3)

        // Select the second worker natively, as a user would.
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        let selected = identities[1]
        let workerB = Self.fakeRows[1]

        // Name ascending: `helper` first, the two `worker`s tied and kept in identity order.
        NativeActivation.click(buttons[1])
        let byName = [Self.fakeRows[2], Self.fakeRows[0], Self.fakeRows[1]]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by name") {
            Self.nativeRows(table) == byName && buttons[1].title == "Sort by Name \u{2191}"
        }
        #expect(Self.modelRows(applier) == byName)
        #expect(Set(Self.modelItemIDs(applier)) == Set(identities), "sorting creates and destroys no row")
        #expect(table.selectedRowIndexes == IndexSet(integer: 2), "the selection followed its process")
        #expect(Self.modelItemIDs(applier)[2] == selected)
        let selectedRow = table.selectedRow
        try #require(selectedRow >= 0)
        #expect(Self.nativeRows(table)[selectedRow] == workerB)

        // A second click reverses it; the selection follows again.
        NativeActivation.click(buttons[1])
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by name, descending") {
            Self.nativeRows(table) == Self.fakeRows && buttons[1].title == "Sort by Name \u{2193}"
        }
        #expect(table.selectedRowIndexes == IndexSet(integer: 1))
        #expect(Self.modelItemIDs(applier) == identities)

        // CPU, descending by default: the rows are already in that order and stay put; only the
        // two buttons whose meaning changed are relabelled, and the unavailable share is last.
        let revision = applier.lastAppliedRevision
        NativeActivation.click(buttons[3])
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by CPU") {
            buttons[3].title == "Sort by CPU \u{2193}" && buttons[1].title == "Sort by Name"
        }
        #expect(applier.lastAppliedRevision.value == revision.value + 1)
        #expect(Self.nativeRows(table) == Self.fakeRows)
        #expect(table.selectedRowIndexes == IndexSet(integer: 1))

        // Every reorder happened inside the native controls built once.
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(scroll.documentView as? NSTableView === table)
        for (handle, id) in zip(buttonHandles, Self.sortButtonIDs) {
            #expect(renderer.registry.handle(for: NodeId(id)) === handle, "sort button \(id) was rebuilt")
        }
        print("PX-009 native SSH evidence: window \(window.windowNumber); buttons=\(buttons.map(\.title)); rows=\(Self.nativeRows(table)); itemIDs=\(Self.modelItemIDs(applier)); selected item \(selected) at row \(table.selectedRow); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-010: the filter input is a standard native NSTextField; editing it sends `TEXT_EDIT`,
    /// the server filters its own rows and updates the filter status line, and the generic client
    /// shows the filtered rows in the same native table with the selection still on the same process,
    /// because the rows keep their item IDs server-side and the client anchors its selection to them.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func filterInputFiltersTheNativeTableAndPreservesSelection() async throws {
        let harness = try await Self.launch(arguments: ["--fake-source"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        renderer.textEditingSession.debounceNanoseconds = 0
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with filter controls") {
            applier.lastAppliedRevision == Revision(1)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.filterInputID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        let filterHandle = try #require(renderer.registry.handle(for: NodeId(Self.filterInputID)))
        let filterField = try #require(filterHandle.view as? NSTextField)
        let filterAdapter = try #require(filterHandle.textAdapter)
        let statusHandle = try #require(renderer.registry.handle(for: NodeId(Self.filterStatusID)))
        let statusField = try #require(statusHandle.view as? NSTextField)

        #expect(filterField.isEditable)
        #expect(filterField.placeholderString == "Filter by name or command (| for OR)")
        #expect(filterField.stringValue == "")
        #expect(statusField.stringValue == "Filter: none · showing all 3 rows")
        #expect(Self.nativeRows(table) == Self.fakeRows)
        let identities = Self.modelItemIDs(applier)
        #expect(identities.count == 3)

        // Select helper natively (row 2).
        table.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        let selected = identities[2]
        let helperRow = Self.fakeRows[2]

        // Filter for "helper": matches only helper, moving it from index 2 to index 0.
        filterField.stringValue = "helper"
        filterAdapter.notifyTextDidChangeForTests()
        filterAdapter.notifyEndEditingForTests()

        let byHelper = [helperRow]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "filter by helper") {
            Self.nativeRows(table) == byHelper && statusField.stringValue.contains("\"helper\" · 1 of 3 rows matched")
        }
        #expect(Self.modelRows(applier) == byHelper)
        #expect(Self.modelItemIDs(applier) == [selected])
        // The selection followed helper from row 2 to row 0!
        #expect(table.selectedRowIndexes == IndexSet(integer: 0))
        #expect(table.selectedRow == 0)
        #expect(Self.nativeRows(table)[0] == helperRow)

        // Clear filter: all rows return and helper is back at row 2 with selection intact.
        filterField.stringValue = ""
        filterAdapter.notifyTextDidChangeForTests()
        filterAdapter.notifyEndEditingForTests()

        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "cleared filter") {
            Self.nativeRows(table) == Self.fakeRows && statusField.stringValue.contains("showing all 3 rows")
        }
        #expect(Self.modelItemIDs(applier) == identities)
        #expect(table.selectedRowIndexes == IndexSet(integer: 2))
        #expect(table.selectedRow == 2)
        #expect(Self.nativeRows(table)[2] == helperRow)

        // Filter for "worker": matches both worker processes (rows 0 and 1). Helper is filtered out.
        filterField.stringValue = "worker"
        filterAdapter.notifyTextDidChangeForTests()
        filterAdapter.notifyEndEditingForTests()

        let byWorker = [Self.fakeRows[0], Self.fakeRows[1]]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "filter by worker") {
            Self.nativeRows(table) == byWorker && statusField.stringValue.contains("\"worker\" · 2 of 3 rows matched")
        }
        #expect(Self.modelRows(applier) == byWorker)
        #expect(Self.modelItemIDs(applier) == [identities[0], identities[1]])

        // Clear filter again: ItemIds remain completely stable.
        filterField.stringValue = ""
        filterAdapter.notifyTextDidChangeForTests()
        filterAdapter.notifyEndEditingForTests()

        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "cleared filter again") {
            Self.nativeRows(table) == Self.fakeRows && statusField.stringValue.contains("showing all 3 rows")
        }
        #expect(Self.modelItemIDs(applier) == identities)

        // Native handles were built once and kept throughout filtering.
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.filterInputID)) === filterHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.filterStatusID)) === statusHandle)

        print("PX-010 native SSH evidence: window \(window.windowNumber); filterField=\(filterField.stringValue); status=\(statusField.stringValue); rows=\(Self.nativeRows(table)); itemIDs=\(Self.modelItemIDs(applier)); selected item \(selected) at row \(table.selectedRow); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-015: Pause view is a standard native button. While the view is paused the scripted
    /// source keeps advancing on the server's clock, yet the native table, every summary line and
    /// the client's revision stay exactly where the pause left them; the filter still works on the
    /// frozen rows and commits its own state; an invalid interval is refused with its reason; and
    /// Resume view converges on a current sample in the same native table, rows keeping their
    /// item IDs.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func pauseFreezesTheNativeTableWhileControlsStillUpdateAndResumeConverges() async throws {
        let harness = try await Self.launch(arguments: ["--fake-sequence", "--refresh-interval-ms", "300"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        renderer.textEditingSession.debounceNanoseconds = 0
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with sampling controls") {
            applier.lastAppliedRevision.value >= 1
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.samplingStatusID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let table = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let pause = try #require(renderer.registry.handle(for: NodeId(Self.samplingPauseID))?.view as? NSButton)
        let refresh = try #require(renderer.registry.handle(for: NodeId(Self.samplingRefreshID))?.view as? NSButton)
        let apply = try #require(renderer.registry.handle(for: NodeId(Self.intervalApplyID))?.view as? NSButton)
        let intervalHandle = try #require(renderer.registry.handle(for: NodeId(Self.intervalInputID)))
        let intervalField = try #require(intervalHandle.view as? NSTextField)
        let intervalAdapter = try #require(intervalHandle.textAdapter)
        let status = try #require(renderer.registry.handle(for: NodeId(Self.samplingStatusID))?.view as? NSTextField)
        let filterHandle = try #require(renderer.registry.handle(for: NodeId(Self.filterInputID)))
        let filterField = try #require(filterHandle.view as? NSTextField)
        let filterAdapter = try #require(filterHandle.textAdapter)
        let filterStatus = try #require(renderer.registry.handle(for: NodeId(Self.filterStatusID))?.view as? NSTextField)
        let summary = try Self.summaryLines(renderer)
        let freshness = try #require(summary.last)

        #expect(pause.title == "Pause view")
        #expect(refresh.title == "Refresh now" && refresh.isEnabled)
        #expect(apply.title == "Apply interval")
        #expect(intervalField.stringValue == "300")
        #expect(status.stringValue == "Live · sampling every 300 ms")
        // The script is running: its freshness line moves with every sample.
        let firstFreshness = freshness.stringValue
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "a live sample") {
            freshness.stringValue != firstFreshness
        }

        NativeActivation.click(pause)
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the paused view") {
            status.stringValue.hasPrefix("Paused at ")
                && status.stringValue.contains("collection stopped")
                && pause.title == "Resume view"
                && !refresh.isEnabled
                && freshness.stringValue.hasPrefix("Paused, not live · ")
        }
        // The pause landed; let a scan that was already running return and be discarded.
        try await Task.sleep(for: .milliseconds(400))
        let frozenRevision = applier.lastAppliedRevision
        let frozenRows = Self.nativeRows(table)
        let frozenModel = Self.modelRows(applier)
        let frozenIDs = Self.modelItemIDs(applier)
        let frozenSummary = summary.map(\.stringValue)
        #expect(frozenModel == frozenRows)
        // Four intervals: the server's script would have published four samples.
        try await Task.sleep(for: .milliseconds(1200))
        #expect(applier.lastAppliedRevision == frozenRevision, "a paused view sends no app mutation")
        #expect(Self.nativeRows(table) == frozenRows)
        #expect(Self.modelItemIDs(applier) == frozenIDs)
        #expect(summary.map(\.stringValue) == frozenSummary)

        // The filter still works, on the frozen rows, and commits its own status.
        filterField.stringValue = "worker"
        filterAdapter.notifyTextDidChangeForTests()
        filterAdapter.notifyEndEditingForTests()
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "a filter applied while paused") {
            filterStatus.stringValue.contains("\"worker\"")
        }
        let filtered = Self.nativeRows(table)
        #expect(!filtered.isEmpty && filtered.allSatisfy { frozenRows.contains($0) })
        #expect(frozenRows.filter { $0[1] == "worker" }.allSatisfy { filtered.contains($0) })
        #expect(applier.lastAppliedRevision.value > frozenRevision.value, "control traffic flows while paused")
        filterField.stringValue = ""
        filterAdapter.notifyTextDidChangeForTests()
        filterAdapter.notifyEndEditingForTests()
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the frozen rows restored") {
            Self.nativeRows(table) == frozenRows && filterStatus.stringValue.contains("showing all")
        }
        #expect(Self.modelItemIDs(applier) == frozenIDs)
        #expect(summary.map(\.stringValue).dropLast() == frozenSummary.dropLast())
        #expect(freshness.stringValue.hasPrefix("Paused, not live · "))

        // An invalid interval is refused with its reason; the interval is kept.
        intervalField.stringValue = "fast"
        intervalAdapter.notifyTextDidChangeForTests()
        intervalAdapter.notifyEndEditingForTests()
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the interval text kept by the server") {
            Self.modelValue(applier, NodeId(Self.intervalInputID)) == "fast"
        }
        NativeActivation.click(apply)
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the refused interval") {
            status.stringValue.contains("interval \"fast\" refused: not a whole number of milliseconds")
                && status.stringValue.contains("keeping 300 ms")
                && status.stringValue.hasPrefix("Paused at ")
        }
        #expect(Self.nativeRows(table) == frozenRows)
        let refusedStatus = status.stringValue

        // Editing the field again clears the refusal of the text it replaced.
        intervalField.stringValue = "300"
        intervalAdapter.notifyTextDidChangeForTests()
        intervalAdapter.notifyEndEditingForTests()
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the refusal cleared") {
            !status.stringValue.contains("refused") && status.stringValue.hasPrefix("Paused at ")
        }
        // Resume: one current sample in the same native table.
        NativeActivation.click(pause)
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the resumed view") {
            pause.title == "Pause view"
                && refresh.isEnabled
                && !status.stringValue.hasPrefix("Paused")
                && !freshness.stringValue.hasPrefix("Paused")
                && !freshness.stringValue.hasPrefix("Resuming")
                && freshness.stringValue != frozenSummary.last
        }
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the converged native table") {
            Self.nativeRows(table) == Self.modelRows(applier) && status.stringValue == "Live · sampling every 300 ms"
        }
        // Rows that survived the pause kept their item IDs.
        let resumedIDs = Set(Self.modelItemIDs(applier))
        #expect(!resumedIDs.isDisjoint(with: frozenIDs), "surviving rows keep their item IDs")
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        print("PX-015 native SSH evidence: window \(window.windowNumber); frozen at client revision \(frozenRevision.value) for 1.2 s with no app mutation; frozenRows=\(frozenRows); filtered while paused=\(filtered); refused=\(refusedStatus); resumed status=\(status.stringValue); freshness=\(freshness.stringValue); rows=\(Self.nativeRows(table)); itemIDs=\(Self.modelItemIDs(applier)); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-016: the basic workflow without a mouse. The explorer publishes the standard `shortcut`
    /// property (PX-016-G01) on its existing controls, and the unchanged generic client binds
    /// them. Real key-down `NSEvent`s go through the window's `performKeyEquivalent(with:)`, and
    /// typing, arrows, paging and Delete through the first responder's own `keyDown(with:)`, as
    /// AppKit delivers them. The test host never activates, so the router's key-window seam is
    /// set, as in G01's native tests; every other guard is the real one. The fixture lists 60
    /// processes, three named exactly `worker` at rows 10, 30 and 50.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func theBasicWorkflowRunsFromTheKeyboardWithDiscoverableAccessibleShortcuts() async throws {
        let harness = try await Self.launch(arguments: ["--fake-repeated-names"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        renderer.textEditingSession.debounceNanoseconds = 0
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with its shortcuts") {
            applier.lastAppliedRevision.value >= 1
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.samplingStatusID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        SurfacePresentation.forHostApplication().present(window)
        window.contentView?.layoutSubtreeIfNeeded()
        let router = renderer.layoutRenderer.shortcutRouter
        router.isWindowKey = { $0 === window }
        func view<T>(_ id: UInt64, as _: T.Type = T.self) throws -> T {
            try #require(renderer.registry.handle(for: NodeId(id))?.view as? T, "node \(id)")
        }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let table = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let searchField: NSTextField = try view(Self.searchInputID)
        let filterField: NSTextField = try view(Self.filterInputID)
        let searchStatus: NSTextField = try view(Self.searchStatusID)
        let filterStatus: NSTextField = try view(Self.filterStatusID)
        let samplingStatus: NSTextField = try view(Self.samplingStatusID)
        let refresh: NSButton = try view(Self.samplingRefreshID)
        let pause: NSButton = try view(Self.samplingPauseID)
        let sortByPID: NSButton = try view(Self.sortButtonIDs[0])
        let sortByName: NSButton = try view(Self.sortButtonIDs[1])
        let inspectorStatus: NSTextField = try view(Self.inspectorLineIDs[0])
        let commandScroll: NSScrollView = try view(Self.inspectorCommandID)
        let commandText = try #require(commandScroll.documentView as? NSTextView)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "all 60 rows") {
            table.numberOfRows == 60 && refresh.isEnabled
        }
        let initialRows = Self.nativeRows(table)
        let identities = Self.modelItemIDs(applier)

        // Discoverable and accessible, in reading order: every shortcut is bound, none is
        // reserved, and each control states its label, its enabled state and its shortcut.
        var reading: [NSObject] = []
        for binding in Self.keyboardShortcuts {
            let handle = try #require(renderer.registry.handle(for: NodeId(binding.id)), "node \(binding.id)")
            #expect(handle.shortcutDiagnostic == nil, "\(binding.label): \(String(describing: handle.shortcutDiagnostic))")
            #expect(handle.shortcutBinding?.shortcut.canonical == binding.shortcut, "\(binding.label)")
            #expect(handle.accessibilityMetadata.shortcut == binding.display, "\(binding.label)")
            let element = handle.view
            #expect(element.accessibilityLabel() == binding.label, "\(binding.label) is announced by its label")
            #expect(element.toolTip == "\(binding.label) (\(binding.display))")
            #expect(element.accessibilityHelp()?.hasSuffix("Keyboard shortcut \(binding.display)") == true,
                    "\(binding.label): help \(String(describing: element.accessibilityHelp()))")
            if let inner = (element as? NSScrollView)?.documentView {
                // The focused table or text announces the shortcut too.
                #expect(inner.accessibilityHelp()?.hasSuffix("Keyboard shortcut \(binding.display)") == true, "\(binding.label)")
            }
            if let control = element as? NSControl {
                #expect(control.isAccessibilityEnabled() == binding.initiallyEnabled,
                        "\(binding.label) is announced as \(binding.initiallyEnabled ? "enabled" : "disabled")")
                #expect(control.isEnabled == binding.initiallyEnabled)
            }
            reading.append(element)
        }
        #expect(renderer.registry.allHandles.allSatisfy { $0.shortcutDiagnostic == nil }, "no shortcut is refused")
        let order = Self.accessibilityOrder(of: reading, under: try #require(window.contentView))
        #expect(order == Array(reading.indices),
                "the accessibility tree reads the controls in the published order: \(order)")

        // A shortcut whose control is disabled does nothing: nothing is searched for yet.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("g", [.command], keyCode: 5, in: window)) == false)
        #expect(router.lastRefusal == .disabled)

        // Find: Command-F focuses the search field; letters typed there are text, not shortcuts.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("f", [.command], keyCode: 3, in: window)))
        let searchEditor = try #require(window.firstResponder as? NSTextView)
        #expect(searchEditor.delegate === searchField, "the search field owns input")
        // An input method's composition owns input: even an enabled control's shortcut waits.
        searchEditor.setMarkedText("かな", selectedRange: NSRange(location: 2, length: 0),
                                   replacementRange: searchEditor.selectedRange)
        #expect(searchEditor.hasMarkedText())
        let refreshKey = try Self.keyEvent("r", [.command], keyCode: 15, in: window)
        #expect(window.performKeyEquivalent(with: refreshKey) == false)
        #expect(router.lastRefusal == .markedText)
        #expect(searchEditor.hasMarkedText(), "the composition is untouched")
        searchEditor.setMarkedText("", selectedRange: NSRange(location: 0, length: 0),
                                   replacementRange: NSRange(location: NSNotFound, length: 0))
        #expect(!searchEditor.hasMarkedText() && searchEditor.string.isEmpty)
        // Copy stays the text field's: the reserved combination is never the application's.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("c", [.command], keyCode: 8, in: window)) == false)
        try Self.typeKeys("worker", in: window)
        // The whole query, not a prefix typed on the way to it, has been searched for.
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the first match") {
            searchStatus.stringValue.contains("\"worker\": 3 matches in 60 shown rows · match 1 of 3 at row 10 of 60")
                && table.selectedRowIndexes == IndexSet(integer: 9)
        }
        #expect(searchField.stringValue == "worker" || searchEditor.string == "worker")
        // Command-G and Shift-Command-G step through the matches while the field keeps focus.
        var selections = [table.selectedRow]
        let steps: [(modifiers: NSEvent.ModifierFlags, nth: Int, row: Int)] = [
            ([.command], 2, 29), ([.command], 3, 49), ([.command, .shift], 2, 29),
        ]
        for (modifiers, nth, row) in steps {
            #expect(window.performKeyEquivalent(with: try Self.keyEvent("g", modifiers, keyCode: 5, in: window)))
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "match \(nth) at row \(row + 1)") {
                searchStatus.stringValue.contains("match \(nth) of 3 at row \(row + 1) of 60")
                    && table.selectedRowIndexes == IndexSet(integer: row)
            }
            selections.append(table.selectedRow)
        }
        #expect(selections == [9, 29, 49, 29])

        // Select: Command-L focuses the table; its own arrow keys move the selection, and the
        // inspector follows each native selection. Paging scrolls locally and selects nothing.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("l", [.command], keyCode: 37, in: window)))
        #expect(window.firstResponder === table)
        let down = try Self.keyEvent(Self.functionKey(NSDownArrowFunctionKey), [.numericPad, .function], keyCode: 125, in: window)
        #expect(window.performKeyEquivalent(with: down) == false, "an arrow is the table's, never a shortcut")
        table.keyDown(with: down)
        #expect(table.selectedRowIndexes == IndexSet(integer: 30))
        let row30 = initialRows[30]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the inspector on row 31") {
            inspectorStatus.stringValue.hasPrefix("Selected process: PID \(row30[0]) · \(row30[1])")
        }
        // The last match revealed the end of the table, so page up.
        let visibleBefore = table.visibleRect
        table.keyDown(with: try Self.keyEvent(Self.functionKey(NSPageUpFunctionKey), [.function], keyCode: 116, in: window))
        // AppKit may animate the page scroll.
        try await AsyncTestSupport.eventually(timeout: .seconds(5), description: "the table paged up locally from \(visibleBefore)") {
            table.visibleRect.origin.y < visibleBefore.origin.y
        }
        #expect(table.selectedRowIndexes == IndexSet(integer: 30), "paging moves the viewport, not the selection")
        table.keyDown(with: try Self.keyEvent(Self.functionKey(NSUpArrowFunctionKey), [.numericPad, .function], keyCode: 126, in: window))
        #expect(table.selectedRowIndexes == IndexSet(integer: 29))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the inspector back on the second worker") {
            inspectorStatus.stringValue.hasPrefix("Selected process: PID \(initialRows[29][0]) · worker")
        }

        // Inspect: Command-I focuses the inspector's selectable command line.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("i", [.command], keyCode: 34, in: window)))
        #expect(window.firstResponder === commandText)
        #expect(commandText.isSelectable && !commandText.isEditable)

        // Sort: Command-2 sorts by name, Command-1 back by PID; each is the button's own click.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("2", [.command], keyCode: 19, in: window)))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by name") {
            sortByName.title == "Sort by Name \u{2191}" && Self.nativeRows(table) == Self.modelRows(applier)
                && Self.nativeRows(table) != initialRows
        }
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("1", [.command], keyCode: 18, in: window)))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by PID") {
            sortByPID.title == "Sort by PID \u{2191}" && Self.nativeRows(table) == initialRows
        }

        // Filter: Option-Command-F focuses the filter field; typing filters on the server.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("f", [.command, .option], keyCode: 3, in: window)))
        #expect((window.firstResponder as? NSTextView)?.delegate === filterField)
        try Self.typeKeys("worker", in: window)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the filtered table") {
            filterStatus.stringValue.contains("\"worker\"") && table.numberOfRows == 3
        }
        #expect(Self.nativeRows(table).allSatisfy { $0[1] == "worker" })

        // Reset: refocusing a field selects its whole text, so one Delete clears it.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("l", [.command], keyCode: 37, in: window)))
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("f", [.command, .option], keyCode: 3, in: window)))
        let filterEditor = try #require(window.firstResponder as? NSTextView)
        #expect(filterEditor.selectedRange == NSRange(location: 0, length: 6), "the whole filter is selected")
        try Self.typeKeys("\u{7F}", in: window)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the filter reset") {
            filterStatus.stringValue.contains("showing all") && table.numberOfRows == 60
        }
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("f", [.command], keyCode: 3, in: window)))
        let refocusedSearch = try #require(window.firstResponder as? NSTextView)
        #expect(refocusedSearch.selectedRange == NSRange(location: 0, length: 6), "the whole query is selected")
        try Self.typeKeys("\u{7F}", in: window)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the search reset") {
            searchStatus.stringValue.contains("Search: none")
        }
        #expect(Self.nativeRows(table) == initialRows && Self.modelItemIDs(applier) == identities)
        // With no search, Next is disabled again, and so are both reset buttons with nothing to
        // reset: their shortcuts do nothing.
        let refused: [(key: String, modifiers: NSEvent.ModifierFlags, keyCode: UInt16, what: String)] = [
            ("g", [.command], 5, "Next match"),
            ("u", [.command, .option], 32, "All visible users"),
            ("r", [.command, .option], 15, "Restore default columns"),
        ]
        for (key, modifiers, keyCode, what) in refused {
            #expect(window.performKeyEquivalent(with: try Self.keyEvent(key, modifiers, keyCode: keyCode, in: window)) == false, "\(what)")
            #expect(router.lastRefusal == .disabled, "\(what)")
        }

        // Pause: Option-Command-P pauses; Refresh now is then disabled and Command-R does nothing.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("p", [.command, .option], keyCode: 35, in: window)))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the paused view") {
            samplingStatus.stringValue.hasPrefix("Paused at ") && pause.title == "Resume view" && !refresh.isEnabled
        }
        #expect(!refresh.isAccessibilityEnabled(), "Refresh now is announced as disabled while paused")
        let pausedRevision = applier.lastAppliedRevision
        #expect(window.performKeyEquivalent(with: refreshKey) == false)
        #expect(router.lastRefusal == .disabled)
        try await Task.sleep(for: .milliseconds(600))
        #expect(applier.lastAppliedRevision == pausedRevision, "a refused Command-R sends nothing")
        #expect(samplingStatus.stringValue.hasPrefix("Paused at "))
        // Resume, then Command-R refreshes like the button.
        #expect(window.performKeyEquivalent(with: try Self.keyEvent("p", [.command, .option], keyCode: 35, in: window)))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the resumed view") {
            pause.title == "Pause view" && refresh.isEnabled && !samplingStatus.stringValue.hasPrefix("Paused")
        }
        #expect(window.performKeyEquivalent(with: refreshKey))
        #expect(router.lastRefusal == nil)

        // The native controls were built once and kept throughout.
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        print("PX-016 native SSH evidence: window \(window.windowNumber); shortcuts=\(Self.keyboardShortcuts.map { "\($0.label)=\($0.display)" }); accessibility order=\(order); selections=\(selections); inspector=\(inspectorStatus.stringValue); filter=\(filterStatus.stringValue); search=\(searchStatus.stringValue); sampling=\(samplingStatus.stringValue); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-017: "Follow selected process" is a standard native checkbox whose state the server
    /// owns, beside a status line. The fixture lists 60 processes; under the CPU sort only
    /// `tracked` (PID 6010) moves, to the scripted rank of each sample, and at the sixth sample it
    /// has ended and a new process reuses its PID. Samples are taken only by Refresh now (the
    /// interval is 300 s), so each step is deterministic. With follow on, a resort that moves the
    /// process off-screen reveals it through the generic `keep_visible` hint (PX-017-G01) while the
    /// client's own selection stays on it, and nothing is ever promoted; a sample that does not move
    /// it keeps the user's pan; a retarget moves the hint, and a second client that attaches then
    /// restores no selected row and is revealed to the followed one; with follow off, a resort
    /// moves neither the viewport nor the selection off its process; and when the process ends,
    /// follow stops and says so, and the new process with its PID is neither selected nor revealed.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func followKeepsTheFollowedProcessVisibleAndNeverJumpsOtherwise() async throws {
        let harness = try await Self.launch(arguments: ["--fake-follow", "--refresh-interval-ms", "300000"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with its follow controls") {
            applier.lastAppliedRevision.value >= 1
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.followStatusID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        SurfacePresentation.forHostApplication().present(window)
        window.contentView?.layoutSubtreeIfNeeded()
        // The test host never activates; as in PX-016, this window is the key window.
        let router = renderer.layoutRenderer.shortcutRouter
        router.isWindowKey = { $0 === window }
        func view<T>(_ id: UInt64, as _: T.Type = T.self) throws -> T {
            try #require(renderer.registry.handle(for: NodeId(id))?.view as? T, "node \(id)")
        }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let table = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let toggleHandle = try #require(renderer.registry.handle(for: NodeId(Self.followToggleID)))
        let toggle: NSButton = try view(Self.followToggleID)
        let status: NSTextField = try view(Self.followStatusID)
        let refresh: NSButton = try view(Self.samplingRefreshID)
        let sortByCPU: NSButton = try view(Self.sortButtonIDs[3])
        let inspectorStatus: NSTextField = try view(Self.inspectorLineIDs[0])
        let freshness = try #require(try Self.summaryLines(renderer).last)
        let followKey = try Self.keyEvent("f", [.command, .shift], keyCode: 3, in: window)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "all 60 rows") {
            table.numberOfRows == 60 && refresh.isEnabled
        }
        func isVisible(_ row: Int) -> Bool {
            NSLocationInRange(row, table.rows(in: table.visibleRect))
        }
        func trackedRow() -> Int? {
            Self.nativeRows(table).firstIndex { $0.first == "6010" }
        }
        /// Where the table is scrolled to: the vertical offset and the rows it shows. Column
        /// widths may settle as cells change, so the width is not part of it.
        func viewport() -> (offset: CGFloat, rows: NSRange) {
            (table.visibleRect.origin.y, table.rows(in: table.visibleRect))
        }
        var trace: [String] = []
        /// Takes one sample with Refresh now and waits for it to be drawn and for `done`.
        func sample(_ step: String, until done: @escaping @MainActor () -> Bool) async throws {
            let before = freshness.stringValue
            NativeActivation.click(refresh)
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: step) {
                freshness.stringValue != before
                    && Self.nativeRows(table) == Self.modelRows(applier)
                    && done()
            }
            trace.append("\(step): rows \(table.rows(in: table.visibleRect)), selected \(Array(table.selectedRowIndexes)) · \(status.stringValue)")
        }

        // A standard checkbox with its label, its shortcut and its status; follow starts off.
        #expect(toggle.title == "Follow selected process" && toggle.state == .off && toggle.isEnabled)
        #expect(toggleHandle.shortcutBinding?.shortcut.canonical == "primary+shift+f")
        #expect(toggle.toolTip == "Follow selected process (⇧⌘F)")
        #expect(status.stringValue == Self.followOffStatus)

        // Nothing is selected: ⇧⌘F flips the checkbox locally, the server refuses with its
        // reason and puts the checkbox back.
        #expect(window.performKeyEquivalent(with: followKey))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the refusal") {
            status.stringValue == "Follow: off · nothing is selected; select a row, then turn follow on"
                && toggle.state == .off
        }

        // Sort by CPU: `tracked` is the busiest. Select it locally, then follow it.
        NativeActivation.click(sortByCPU)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by CPU") {
            sortByCPU.title == "Sort by CPU \u{2193}" && trackedRow() == 0
                && Self.nativeRows(table) == Self.modelRows(applier)
        }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the inspector on tracked") {
            inspectorStatus.stringValue.hasPrefix("Selected process: PID 6010 · tracked")
        }
        let identities = Set(Self.modelItemIDs(applier))
        let trackedID = Self.modelItemIDs(applier)[0]
        #expect(window.performKeyEquivalent(with: followKey))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "following tracked") {
            toggle.state == .on && status.stringValue.hasPrefix("Following PID 6010 · tracked · row 1 of 60 · ")
                && applier.store.getNode(NodeId(5))?.keepVisible == ItemId(trackedID)
        }
        #expect(applier.store.getNode(NodeId(5))?.promotedSelection == nil, "turning follow on promotes nothing")
        #expect(table.selectedRowIndexes == IndexSet(integer: 0))
        table.scrollRowToVisible(0)
        #expect(!isVisible(40), "the viewport is shorter than the moves it must follow: \(table.rows(in: table.visibleRect))")

        // A CPU resort moves it to row 56, off-screen: it stays selected and is revealed, and no
        // row is created.
        try await sample("resorted to row 56") {
            trackedRow() == 55 && table.selectedRowIndexes == IndexSet(integer: 55) && isVisible(55)
                && status.stringValue.contains("row 56 of 60")
        }
        #expect(table.numberOfRows == 60)
        #expect(Set(Self.modelItemIDs(applier)) == identities && Self.modelItemIDs(applier)[55] == trackedID,
                "the followed row is the same item, and no row was created")
        #expect(applier.store.getNode(NodeId(5))?.keepVisible == ItemId(trackedID))
        #expect(applier.store.getNode(NodeId(5))?.promotedSelection == nil, "follow never promotes")

        // Pan away: the selection is unchanged, and a sample that does not move the process keeps
        // the viewport where the user put it, with follow still on.
        table.scrollRowToVisible(0)
        let panned = viewport()
        #expect(!isVisible(55) && table.selectedRowIndexes == IndexSet(integer: 55), "panning selects nothing")
        try await sample("the same ranks") { true }
        #expect(viewport() == panned, "a pan is kept while the followed process stays put")
        #expect(table.selectedRowIndexes == IndexSet(integer: 55))
        #expect(toggle.state == .on && status.stringValue.contains("row 56 of 60"))

        // It moves again, to row 41: follow brings it back into view.
        try await sample("resorted to row 41") {
            trackedRow() == 40 && table.selectedRowIndexes == IndexSet(integer: 40) && isVisible(40)
        }
        #expect(viewport() != panned)

        // Follow off, then pan to the top: a resort to row 51 moves neither the viewport nor the
        // selection off its process.
        #expect(window.performKeyEquivalent(with: followKey))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "follow off") {
            toggle.state == .off && status.stringValue == Self.followOffStatus
        }
        table.scrollRowToVisible(0)
        let top = viewport()
        try await sample("resorted to row 51 with follow off") { trackedRow() == 50 }
        #expect(viewport() == top, "non-follow mode does not jump the viewport")
        #expect(table.selectedRowIndexes == IndexSet(integer: 50), "the selection stays on its process")
        #expect(!isVisible(50))

        // Follow on again: the process is revealed at once.
        #expect(window.performKeyEquivalent(with: followKey))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "following again") {
            toggle.state == .on && isVisible(50) && table.selectedRowIndexes == IndexSet(integer: 50)
                && status.stringValue.contains("row 51 of 60")
        }

        // Retarget: select a visible neighbour while following. Follow moves its hint to it and
        // promotes nothing, so nothing moves for this client, and a second client that attaches
        // now restores no selected row, never `tracked`, and is revealed to the neighbour (PR #98
        // review).
        let neighbour = isVisible(49) ? 49 : 51
        #expect(isVisible(neighbour))
        let neighbourPID = Self.nativeRows(table)[neighbour][0]
        let neighbourID = Self.modelItemIDs(applier)[neighbour]
        let beforeRetarget = viewport()
        table.selectRowIndexes(IndexSet(integer: neighbour), byExtendingSelection: false)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "retargeted to row \(neighbour + 1)") {
            status.stringValue.hasPrefix("Following PID \(neighbourPID) · ")
                && applier.store.getNode(NodeId(5))?.keepVisible == ItemId(neighbourID)
        }
        #expect(applier.store.getNode(NodeId(5))?.promotedSelection == nil)
        #expect(table.selectedRowIndexes == IndexSet(integer: neighbour), "the user's selection is kept")
        #expect(viewport() == beforeRetarget, "the clicking client does not move")
        let (secondController, secondApplier, secondRenderer) =
            Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await secondController.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "a second client") {
            secondController.isEventDispatchEnabled && secondRenderer.registry.handle(for: NodeId(5)) != nil
                && secondApplier.lastAppliedRevision == applier.lastAppliedRevision
        }
        let secondWindow = try #require(secondRenderer.registry.handle(for: NodeId(1))?.window)
        SurfacePresentation.forHostApplication().present(secondWindow)
        secondWindow.contentView?.layoutSubtreeIfNeeded()
        let secondTable = try #require(
            (secondRenderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView)
        let secondStatus = try #require(secondRenderer.registry.handle(for: NodeId(Self.followStatusID))?.view as? NSTextField)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the second client's table, revealed to the neighbour") {
            secondTable.numberOfRows == 60 && secondStatus.stringValue.hasPrefix("Following PID \(neighbourPID) · ")
                && NSLocationInRange(neighbour, secondTable.rows(in: secondTable.visibleRect))
        }
        #expect(secondApplier.store.getNode(NodeId(5))?.promotedSelection == nil)
        #expect(secondApplier.store.getNode(NodeId(5))?.keepVisible == ItemId(neighbourID))
        #expect(secondTable.selectedRowIndexes.isEmpty, "a fresh client restores no row, never the old target")
        let secondVisible = secondTable.rows(in: secondTable.visibleRect)
        await secondController.stop()
        secondWindow.orderOut(nil)
        secondWindow.close()
        trace.append("retargeted to row \(neighbour + 1) (PID \(neighbourPID)); a second client selected [] and shows rows \(secondVisible)")
        // Back on tracked, as the next step expects.
        table.selectRowIndexes(IndexSet(integer: 50), byExtendingSelection: false)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "following tracked again") {
            status.stringValue.hasPrefix("Following PID 6010 · tracked · row 51 of 60")
                && applier.store.getNode(NodeId(5))?.keepVisible == ItemId(trackedID)
        }
        #expect(table.selectedRowIndexes == IndexSet(integer: 50))

        // The process ends and a new one reuses PID 6010 in the same sample: follow stops and
        // says why, and the new process is neither selected nor revealed.
        table.scrollRowToVisible(59)
        let bottom = viewport()
        try await sample("the followed process ended") {
            status.stringValue.hasPrefix("Follow stopped: PID 6010 · tracked has ended · a new process that reuses its PID is not followed")
                && toggle.state == .off
        }
        let reusedRow = try #require(trackedRow())
        #expect(reusedRow == 2)
        #expect(Self.modelItemIDs(applier)[reusedRow] != trackedID, "the reused PID is another item")
        #expect(!Self.modelItemIDs(applier).contains(trackedID))
        #expect(table.selectedRowIndexes.isEmpty, "nothing is selected for the new process")
        #expect(viewport() == bottom, "nothing scrolled to it")
        #expect(applier.store.getNode(NodeId(5))?.promotedSelection == nil)
        #expect(applier.store.getNode(NodeId(5))?.keepVisible == nil, "the hint is cleared")
        #expect(inspectorStatus.stringValue.contains("this process has ended"))
        try await sample("after the end") { true }
        #expect(table.selectedRowIndexes.isEmpty && viewport() == bottom && toggle.state == .off)

        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.followToggleID)) === toggleHandle)
        print("PX-017 native SSH evidence: window \(window.windowNumber); trackedID=\(trackedID); trace=\(trace); status=\(status.stringValue); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-017-G02 (Codex review of PR #101): a user deselect is reported as `SELECTION_CHANGED`
    /// with no `value`. `deselectAll(nil)` goes through the table's delegate exactly as a click in
    /// empty space does. While following, the deselect stops follow with its reason and clears the
    /// hint, and the inspector says the selection was cleared; turning follow on afterwards is
    /// refused rather than following the deselected process. The same holds for select, deselect,
    /// then follow on, with follow never on before.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func deselectingStopsFollowClearsTheInspectorAndFollowOnIsRefused() async throws {
        let harness = try await Self.launch(arguments: ["--fake-follow", "--refresh-interval-ms", "300000"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with its follow controls") {
            applier.lastAppliedRevision.value >= 1
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.followStatusID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        SurfacePresentation.forHostApplication().present(window)
        window.contentView?.layoutSubtreeIfNeeded()
        let router = renderer.layoutRenderer.shortcutRouter
        router.isWindowKey = { $0 === window }
        func view<T>(_ id: UInt64, as _: T.Type = T.self) throws -> T {
            try #require(renderer.registry.handle(for: NodeId(id))?.view as? T, "node \(id)")
        }
        let table = try #require((renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView)
        let toggle: NSButton = try view(Self.followToggleID)
        let status: NSTextField = try view(Self.followStatusID)
        let inspectorStatus: NSTextField = try view(Self.inspectorLineIDs[0])
        let followKey = try Self.keyEvent("f", [.command, .shift], keyCode: 3, in: window)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "all 60 rows") {
            table.numberOfRows == 60
        }
        let cleared = "Selected process: none · the selection was cleared; select a row to inspect it"
        let refused = "Follow: off · nothing is selected; select a row, then turn follow on"

        // Follow on, then deselect: follow stops with its reason and its hint is cleared.
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        let first = Self.nativeRows(table)[0][0]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the inspector on row 1") {
            inspectorStatus.stringValue.hasPrefix("Selected process: PID \(first) · ")
        }
        #expect(window.performKeyEquivalent(with: followKey))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "following row 1") {
            toggle.state == .on && status.stringValue.hasPrefix("Following PID \(first) · ")
                && applier.store.getNode(NodeId(5))?.keepVisible != nil
        }
        table.deselectAll(nil)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "follow stopped by the deselect") {
            status.stringValue == "Follow stopped: the selection was cleared; select a row and turn follow on again"
                && toggle.state == .off
                && applier.store.getNode(NodeId(5))?.keepVisible == nil
                && inspectorStatus.stringValue == cleared
        }
        #expect(table.selectedRowIndexes.isEmpty, "the server selected nothing back")
        #expect(applier.store.getNode(NodeId(5))?.promotedSelection == nil)

        // Turning follow on now is refused: nothing is selected.
        #expect(window.performKeyEquivalent(with: followKey))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "follow on refused after the clear") {
            status.stringValue == refused && toggle.state == .off
        }
        #expect(applier.store.getNode(NodeId(5))?.keepVisible == nil)

        // Select, deselect, then follow on, with follow off throughout: refused, never the old row.
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        let second = Self.nativeRows(table)[1][0]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the inspector on row 2") {
            inspectorStatus.stringValue.hasPrefix("Selected process: PID \(second) · ")
        }
        table.deselectAll(nil)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the inspector cleared") {
            inspectorStatus.stringValue == cleared
        }
        let beforeToggle = applier.lastAppliedRevision.value
        #expect(window.performKeyEquivalent(with: followKey))
        // The refusal text is already shown, so wait for the server's answer to this press: it
        // always states the toggle's value, in a new revision.
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "follow on refused again") {
            applier.lastAppliedRevision.value > beforeToggle && status.stringValue == refused && toggle.state == .off
        }
        #expect(applier.store.getNode(NodeId(5))?.keepVisible == nil, "the deselected process is not followed")
        #expect(table.selectedRowIndexes.isEmpty)
        print("PX-017-G02 native SSH evidence: window \(window.windowNumber); first=\(first); second=\(second); follow=\(status.stringValue); inspector=\(inspectorStatus.stringValue); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-017 on `keep_visible`: rapid arrow keys while following are never overwritten, even
    /// when a sample lands mid-burst. Under the CPU sort, each round starts on row 36 and presses
    /// Down 20 times (real key-down events into the native table), clicking Refresh now after the
    /// tenth press; the fixture's samples move `tracked` across rows 40–56 (from the top in the
    /// first round), shifting the rows the burst walks through. Before every press the selection
    /// is still the row the previous press chose, the burst ends on the last press, nothing is ever
    /// promoted, and the hint ends on the last press, which is visible.
    @Test(.timeLimit(.minutes(2)))
    @MainActor
    func rapidArrowKeysWhileFollowingEndOnTheLastPressWithASampleMidBurst() async throws {
        let harness = try await Self.launch(arguments: ["--fake-follow", "--refresh-interval-ms", "300000"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with its follow controls") {
            applier.lastAppliedRevision.value >= 1
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.followStatusID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        SurfacePresentation.forHostApplication().present(window)
        window.contentView?.layoutSubtreeIfNeeded()
        let router = renderer.layoutRenderer.shortcutRouter
        router.isWindowKey = { $0 === window }
        func view<T>(_ id: UInt64, as _: T.Type = T.self) throws -> T {
            try #require(renderer.registry.handle(for: NodeId(id))?.view as? T, "node \(id)")
        }
        let table = try #require((renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView)
        let status: NSTextField = try view(Self.followStatusID)
        let refresh: NSButton = try view(Self.samplingRefreshID)
        let sortByCPU: NSButton = try view(Self.sortButtonIDs[3])
        let inspectorStatus: NSTextField = try view(Self.inspectorLineIDs[0])
        let freshness = try #require(try Self.summaryLines(renderer).last)
        let followKey = try Self.keyEvent("f", [.command, .shift], keyCode: 3, in: window)
        let down = try Self.keyEvent(Self.functionKey(NSDownArrowFunctionKey), [.numericPad, .function], keyCode: 125, in: window)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "all 60 rows") {
            table.numberOfRows == 60 && refresh.isEnabled
        }
        func selectedPID() -> String? {
            table.selectedRow >= 0 ? Self.nativeRows(table)[table.selectedRow][0] : nil
        }
        func isVisible(_ row: Int) -> Bool {
            NSLocationInRange(row, table.rows(in: table.visibleRect))
        }
        func node() -> Node? { applier.store.getNode(NodeId(5)) }

        NativeActivation.click(sortByCPU)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by CPU") {
            sortByCPU.title == "Sort by CPU \u{2193}" && Self.nativeRows(table).first?.first == "6010"
                && Self.nativeRows(table) == Self.modelRows(applier)
        }
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the inspector on tracked") {
            inspectorStatus.stringValue.hasPrefix("Selected process: PID 6010 · tracked")
        }
        #expect(window.performKeyEquivalent(with: followKey))
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "following tracked") {
            status.stringValue.hasPrefix("Following PID 6010 · tracked · row 1 of 60 · ")
        }

        var results: [String] = []
        for (label, gap) in [("yield", 0), ("1 ms", 1), ("30 ms key repeat", 30), ("80 ms key repeat", 80)] {
            // Start on row 36 with a click: follow retargets to it.
            table.selectRowIndexes(IndexSet(integer: 35), byExtendingSelection: false)
            let startPID = try #require(selectedPID())
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "\(label): following row 36") {
                status.stringValue.hasPrefix("Following PID \(startPID) · ")
            }
            let before = freshness.stringValue
            var lastPressed = startPID
            var pressed: [String] = []
            var displaced: [String] = []
            var promoted = 0
            var selectedHiddenAfterGap = 0
            for press in 1...20 {
                if selectedPID() != lastPressed {
                    displaced.append("press \(press): \(selectedPID() ?? "none") instead of \(lastPressed)")
                }
                window.makeFirstResponder(table)
                table.keyDown(with: down)
                lastPressed = try #require(selectedPID())
                pressed.append(lastPressed)
                if press == 10 { NativeActivation.click(refresh) }
                if gap == 0 { await Task.yield() } else { try await Task.sleep(for: .milliseconds(gap)) }
                if node()?.promotedSelection != nil { promoted += 1 }
                if table.selectedRow >= 0, !isVisible(table.selectedRow) { selectedHiddenAfterGap += 1 }
            }
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "\(label): the sample and the last retarget") {
                freshness.stringValue != before && Self.nativeRows(table) == Self.modelRows(applier)
                    && status.stringValue.hasPrefix("Following PID \(lastPressed) · ")
            }
            // Let any reply still in flight land.
            try await Task.sleep(for: .milliseconds(300))
            let final = selectedPID()
            let hinted = node()?.keepVisible
            let finalID = table.selectedRow >= 0 ? ItemId(Self.modelItemIDs(applier)[table.selectedRow]) : nil
            results.append("\(label): pressed \(pressed.count) distinct \(Set(pressed).count), final PID \(final ?? "none"), last PID \(lastPressed), displaced \(displaced), promoted \(promoted), selected row hidden after \(selectedHiddenAfterGap) gaps, final row \(table.selectedRow + 1) visible \(table.selectedRow >= 0 && isVisible(table.selectedRow)), tracked row \((Self.nativeRows(table).firstIndex { $0.first == "6010" } ?? -1) + 1)")
            // A sample can move `tracked` ahead of the burst, so the same process may be pressed
            // twice; every press still moves on from the one before.
            #expect(zip([startPID] + pressed, pressed).allSatisfy { $0 != $1 }, "\(label): each press moved on: \(pressed)")
            #expect(displaced.isEmpty, "\(label): no commit changed the selection: \(displaced)")
            #expect(final == lastPressed, "\(label): the burst ends on the last press")
            #expect(promoted == 0 && node()?.promotedSelection == nil, "\(label): follow never promotes")
            #expect(hinted != nil && hinted == finalID, "\(label): the hint ends on the last press")
            #expect(table.selectedRow >= 0 && isVisible(table.selectedRow), "\(label): the last press is in view")
        }
        print("PX-017 rapid arrows native SSH evidence: \(results)")
        await controller.stop()
    }

    // PX-017: the follow controls.
    fileprivate static let followToggleID: UInt64 = 68
    fileprivate static let followStatusID: UInt64 = 69
    fileprivate static let followOffStatus =
        "Follow: off · the selection stays on its process and the table stays where you scroll it"

    /// PX-016's bindings in the published reading order, with the label, the macOS rendering
    /// and the enabled state of a live, unsearched view at default columns.
    fileprivate struct KeyboardBinding {
        let id: UInt64
        let label: String
        let shortcut: String
        let display: String
        let initiallyEnabled: Bool
    }

    fileprivate static let keyboardShortcuts: [KeyboardBinding] = [
        KeyboardBinding(id: 62, label: "Pause view", shortcut: "primary+alt+p", display: "⌥⌘P", initiallyEnabled: true),
        KeyboardBinding(id: 63, label: "Refresh now", shortcut: "primary+r", display: "⌘R", initiallyEnabled: true),
        KeyboardBinding(id: 26, label: "Filter processes", shortcut: "primary+alt+f", display: "⌥⌘F", initiallyEnabled: true),
        KeyboardBinding(id: 51, label: "All visible users", shortcut: "primary+alt+u", display: "⌥⌘U", initiallyEnabled: false),
        KeyboardBinding(id: 43, label: "Find processes", shortcut: "primary+f", display: "⌘F", initiallyEnabled: true),
        KeyboardBinding(id: 44, label: "Previous match", shortcut: "primary+shift+g", display: "⇧⌘G", initiallyEnabled: false),
        KeyboardBinding(id: 45, label: "Next match", shortcut: "primary+g", display: "⌘G", initiallyEnabled: false),
        KeyboardBinding(id: 21, label: "Sort by PID", shortcut: "primary+1", display: "⌘1", initiallyEnabled: true),
        KeyboardBinding(id: 22, label: "Sort by Name", shortcut: "primary+2", display: "⌘2", initiallyEnabled: true),
        KeyboardBinding(id: 23, label: "Sort by Resident", shortcut: "primary+3", display: "⌘3", initiallyEnabled: true),
        KeyboardBinding(id: 24, label: "Sort by CPU", shortcut: "primary+4", display: "⌘4", initiallyEnabled: true),
        KeyboardBinding(id: 41, label: "Restore default columns", shortcut: "primary+alt+r", display: "⌥⌘R", initiallyEnabled: false),
        // PX-017: the follow checkbox, between the column chooser and the table.
        KeyboardBinding(id: 68, label: "Follow selected process", shortcut: "primary+shift+f", display: "⇧⌘F", initiallyEnabled: true),
        KeyboardBinding(id: 5, label: "Processes", shortcut: "primary+l", display: "⌘L", initiallyEnabled: true),
        KeyboardBinding(id: 60, label: "Command line and executable", shortcut: "primary+i", display: "⌘I", initiallyEnabled: true),
    ]

    /// A real key-down event; named keys match by `keyCode`, characters by the layout.
    @MainActor
    fileprivate static func keyEvent(
        _ characters: String, _ modifiers: NSEvent.ModifierFlags = [], keyCode: UInt16, in window: NSWindow
    ) throws -> NSEvent {
        try #require(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: modifiers, timestamp: 0,
            windowNumber: window.windowNumber, context: nil, characters: characters,
            charactersIgnoringModifiers: characters, isARepeat: false, keyCode: keyCode
        ))
    }

    fileprivate static func functionKey(_ key: Int) -> String {
        String(Character(UnicodeScalar(UInt32(key))!))
    }

    /// US-layout key codes for the characters the keyboard test types.
    fileprivate static let typedKeyCodes: [Character: UInt16] = [
        "w": 13, "o": 31, "r": 15, "k": 40, "e": 14, "\u{7F}": 51,
    ]

    /// Types `text` into the first responder through its own `keyDown(with:)`. Each key is
    /// first offered as a key equivalent, which no plain key is: letters are text.
    @MainActor
    fileprivate static func typeKeys(_ text: String, in window: NSWindow) throws {
        let editor = try #require(window.firstResponder as? NSTextView, "a text editor owns input")
        for character in text {
            let event = try keyEvent(String(character), keyCode: try #require(typedKeyCodes[character]), in: window)
            #expect(window.performKeyEquivalent(with: event) == false, "a typed \(character) is never a shortcut")
            editor.keyDown(with: event)
        }
    }

    /// The order in which a depth-first walk of the native accessibility tree under `root`
    /// reaches each of `targets`: the order an assistive technology reads them. A control is
    /// reached as itself or as its cell, which AppKit exposes in the control's place.
    @MainActor
    fileprivate static func accessibilityOrder(of targets: [NSObject], under root: NSView) -> [Int] {
        var order: [Int] = []
        func visit(_ element: Any, depth: Int) {
            guard let object = element as? NSObject else { return }
            if let index = targets.firstIndex(where: { $0 === object || ($0 as? NSControl)?.cell === object }) {
                order.append(index)
                return
            }
            guard depth < 32, !(object is NSTableView), !(object is NSTextView) else { return }
            let children: [Any]
            if let view = object as? NSView {
                children = view.accessibilityChildren() ?? []
            } else if let element = object as? NSAccessibilityElement {
                children = element.accessibilityChildren() ?? []
            } else {
                children = []
            }
            for child in children { visit(child, depth: depth + 1) }
        }
        visit(root, depth: 0)
        return order
    }

    // PX-015: the sampling controls.
    fileprivate static let samplingPauseID: UInt64 = 62
    fileprivate static let samplingRefreshID: UInt64 = 63
    fileprivate static let intervalInputID: UInt64 = 64
    fileprivate static let intervalApplyID: UInt64 = 65
    fileprivate static let samplingStatusID: UInt64 = 66

    /// The string `id` holds as its `VALUE` on the client.
    @MainActor
    fileprivate static func modelValue(_ applier: TransactionApplier, _ id: NodeId) -> String {
        guard case .string(let text)? = applier.store.getNode(id)?.getProperty(.value) else { return "" }
        return text
    }

    /// PX-011 (with PX-011-G01): search is a standard native text field, two standard buttons
    /// that step between matches, a PID field and a Go to PID button. Every one sends a standard
    /// `TEXT_EDIT` or `ACTIVATE`; the server keeps the current match by item ID, publishes it on
    /// the search status line and promotes it to the table's semantic selection, so the generic
    /// client selects the row and scrolls it into view. The fixture lists 60 processes, three
    /// named exactly `worker` at rows 10, 30 and 50, so the later matches start outside the
    /// viewport. The native table keeps every row throughout: search locates, never hides.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func searchSelectsAndRevealsMatchesWithoutHidingRowsAndJumpsToAPID() async throws {
        let harness = try await Self.launch(arguments: ["--fake-repeated-names"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        renderer.textEditingSession.debounceNanoseconds = 0
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with search controls") {
            applier.lastAppliedRevision == Revision(1)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.searchStatusID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let table = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let searchHandle = try #require(renderer.registry.handle(for: NodeId(Self.searchInputID)))
        let searchField = try #require(searchHandle.view as? NSTextField)
        let searchAdapter = try #require(searchHandle.textAdapter)
        let pidHandle = try #require(renderer.registry.handle(for: NodeId(Self.pidInputID)))
        let pidField = try #require(pidHandle.view as? NSTextField)
        let pidAdapter = try #require(pidHandle.textAdapter)
        let previous = try #require(renderer.registry.handle(for: NodeId(Self.searchPreviousID))?.view as? NSButton)
        let next = try #require(renderer.registry.handle(for: NodeId(Self.searchNextID))?.view as? NSButton)
        let go = try #require(renderer.registry.handle(for: NodeId(Self.pidGoID))?.view as? NSButton)
        let statusHandle = try #require(renderer.registry.handle(for: NodeId(Self.searchStatusID)))
        let statusField = try #require(statusHandle.view as? NSTextField)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "all 60 rows") {
            table.numberOfRows == 60
        }
        let identities = Self.modelItemIDs(applier)
        let initialRows = Self.nativeRows(table)
        #expect(identities.count == 60)
        let workerRows = [9, 29, 49]
        for row in workerRows {
            #expect(initialRows[row][0] == String(5001 + row) && initialRows[row][1] == "worker")
        }
        #expect(initialRows.filter { $0[1] == "worker" }.count == 3)
        let initiallyVisible = table.rows(in: table.visibleRect)
        #expect(initiallyVisible.length > 0, "the table has a viewport")
        #expect(!NSLocationInRange(49, initiallyVisible), "row 50 starts outside the viewport: \(initiallyVisible)")
        #expect(table.selectedRowIndexes.isEmpty)

        #expect(searchField.isEditable && pidField.isEditable)
        #expect(searchField.placeholderString == "Find by name or command (| for OR); rows stay visible")
        #expect(previous.title == "Previous match" && next.title == "Next match" && go.title == "Go to PID")
        #expect(!previous.isEnabled && !next.isEnabled && !go.isEnabled)
        #expect(statusField.stringValue == "Search: none · search never hides rows")

        func type(_ text: String, into field: NSTextField, _ adapter: NativeTextEditorAdapter) {
            field.stringValue = text
            adapter.notifyTextDidChangeForTests()
            adapter.notifyEndEditingForTests()
        }
        /// Every row is still in the native table, in the same order, under the same item IDs.
        func expectEveryRowShown(_ step: String) {
            #expect(table.numberOfRows == 60, "\(step): search hides no row")
            #expect(Self.modelItemIDs(applier) == identities, "\(step)")
        }
        func isVisible(_ row: Int) -> Bool {
            NSLocationInRange(row, table.rows(in: table.visibleRect))
        }
        var trace: [String] = []
        var selections: [Int] = []
        /// Waits for the status to say `fragment` and the native table to select `row`, alone,
        /// with the row scrolled into view.
        func waitFor(_ fragment: String, selected row: Int) async throws {
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "\(fragment) with row \(row) selected and visible") {
                statusField.stringValue.contains(fragment)
                    && table.selectedRowIndexes == IndexSet(integer: row)
                    && isVisible(row)
            }
            trace.append(statusField.stringValue)
            selections.append(table.selectedRow)
            expectEveryRowShown(fragment)
        }

        // Three identical names: a new search selects and reveals the first; Next and Previous
        // move the native selection 1 → 2 → 3 → 1 and back, wrapping in both directions.
        type("worker", into: searchField, searchAdapter)
        try await waitFor("\"worker\": 3 matches in 60 shown rows · match 1 of 3 at row 10 of 60 · PID 5010 · worker", selected: 9)
        #expect(previous.isEnabled && next.isEnabled)
        for (button, nth) in [(next, 2), (next, 3), (next, 1), (previous, 3), (previous, 2), (previous, 1)] {
            NativeActivation.click(button)
            let row = workerRows[nth - 1]
            try await waitFor("match \(nth) of 3 at row \(row + 1) of 60 · PID \(5001 + row) · worker", selected: row)
        }
        #expect(selections == [9, 29, 49, 9, 49, 29, 9])

        // A local click on another row stays local across a commit that promotes nothing, and
        // Next continues from the server's current match (match 1), not from the clicked row.
        table.selectRowIndexes(IndexSet(integer: 3), byExtendingSelection: false)
        type("1", into: pidField, pidAdapter)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID field held") {
            applier.store.getNode(NodeId(Self.pidInputID))?.getProperty(.value) == .string("1") && go.isEnabled
        }
        #expect(table.selectedRowIndexes == IndexSet(integer: 3), "an unrelated commit keeps the local selection")
        NativeActivation.click(next)
        try await waitFor("match 2 of 3 at row 30 of 60 · PID 5030 · worker", selected: 29)

        // No match: a visible explanation, navigation disabled, the native selection unchanged.
        type("zzz", into: searchField, searchAdapter)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "no match explained") {
            statusField.stringValue.contains("no match for \"zzz\" in 60 shown rows; search hides no rows")
                && !previous.isEnabled && !next.isEnabled
        }
        trace.append(statusField.stringValue)
        #expect(table.selectedRowIndexes == IndexSet(integer: 29), "no match leaves the selection unchanged")
        expectEveryRowShown("no match")
        type("", into: searchField, searchAdapter)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "search cleared") {
            statusField.stringValue.contains("Search: none")
        }
        #expect(table.selectedRowIndexes == IndexSet(integer: 29))

        // Jump to a PID: Go to PID is enabled only once the server holds a PID; it selects and
        // reveals the row, here the last worker, back from the top of the table.
        table.scrollRowToVisible(0)
        type("5050", into: pidField, pidAdapter)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID field held") {
            applier.store.getNode(NodeId(Self.pidInputID))?.getProperty(.value) == .string("5050") && go.isEnabled
        }
        NativeActivation.click(go)
        try await waitFor("PID 5050 found at row 50 of 60 · worker", selected: 49)
        type("5004", into: pidField, pidAdapter)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID field held") {
            applier.store.getNode(NodeId(Self.pidInputID))?.getProperty(.value) == .string("5004") && go.isEnabled
        }
        NativeActivation.click(go)
        try await waitFor("PID 5004 found at row 4 of 60 · idle-03", selected: 3)
        // A PID no shown process has is explained, and the selection is unchanged.
        type("9999", into: pidField, pidAdapter)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID field held") {
            applier.store.getNode(NodeId(Self.pidInputID))?.getProperty(.value) == .string("9999") && go.isEnabled
        }
        NativeActivation.click(go)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "missing PID explained") {
            statusField.stringValue.contains("no shown process has PID 9999")
        }
        trace.append(statusField.stringValue)
        #expect(table.selectedRowIndexes == IndexSet(integer: 3))
        // A malformed PID cannot be submitted, and the status says why.
        type("41a", into: pidField, pidAdapter)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "Go to PID disabled") {
            statusField.stringValue.contains("PID field: digits only") && !go.isEnabled
        }
        trace.append(statusField.stringValue)
        #expect(table.selectedRowIndexes == IndexSet(integer: 3))
        expectEveryRowShown("malformed PID")
        #expect(Self.nativeRows(table) == initialRows, "every row kept its cells")

        // Native handles were built once and kept throughout.
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.searchInputID)) === searchHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.searchStatusID)) === statusHandle)
        print("PX-011 native SSH evidence: window \(window.windowNumber); rows=\(table.numberOfRows); initially visible=\(initiallyVisible); selections=\(selections); final selection=\(table.selectedRow) visible=\(table.rows(in: table.visibleRect)); itemIDs unchanged=\(Self.modelItemIDs(applier) == identities); statuses=\(trace); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-012: the user filter and PID set are two standard native buttons, two standard native
    /// text fields and a status line. Every one sends a standard `ACTIVATE` or `TEXT_EDIT`; the
    /// server resolves owners by UID, names them through its own bounded cache, filters the rows
    /// and explains an empty result. The fixture's PID 4101 belongs to its own account (UID
    /// 1000, `alice`), PID 4102 to UID 1001 (`bob`), and the helper's owner is unreadable. The
    /// native table shows exactly the kept rows, each under the item ID it had before, and
    /// clearing the filters restores every row and identity.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func userFilterAndPIDSetFilterTheNativeTableAndKeepItemIDs() async throws {
        let harness = try await Self.launch(arguments: ["--fake-source"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        renderer.textEditingSession.debounceNanoseconds = 0
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with user-filter controls") {
            applier.lastAppliedRevision == Revision(1)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.ownerStatusID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let table = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let mine = try #require(renderer.registry.handle(for: NodeId(Self.ownerMineID))?.view as? NSButton)
        let all = try #require(renderer.registry.handle(for: NodeId(Self.ownerAllID))?.view as? NSButton)
        let userHandle = try #require(renderer.registry.handle(for: NodeId(Self.ownerInputID)))
        let userField = try #require(userHandle.view as? NSTextField)
        let userAdapter = try #require(userHandle.textAdapter)
        let pidSetHandle = try #require(renderer.registry.handle(for: NodeId(Self.pidSetInputID)))
        let pidSetField = try #require(pidSetHandle.view as? NSTextField)
        let pidSetAdapter = try #require(pidSetHandle.textAdapter)
        let statusHandle = try #require(renderer.registry.handle(for: NodeId(Self.ownerStatusID)))
        let statusField = try #require(statusHandle.view as? NSTextField)

        #expect(Self.nativeRows(table) == Self.fakeRows)
        let identities = Self.modelItemIDs(applier)
        #expect(identities.count == 3)
        #expect(mine.title == "Only my processes" && all.title == "All visible users")
        #expect(mine.isEnabled && !all.isEnabled, "every visible user is the starting choice")
        #expect(userField.isEditable && pidSetField.isEditable)
        #expect(userField.placeholderString == "UID or account name")
        #expect(pidSetField.placeholderString == "PIDs, separated by commas or spaces")
        #expect(statusField.stringValue == "User: all users visible to this daemon's account · showing all 3 processes")

        func type(_ text: String, into field: NSTextField, _ adapter: NativeTextEditorAdapter) {
            field.stringValue = text
            adapter.notifyTextDidChangeForTests()
            adapter.notifyEndEditingForTests()
        }
        var trace: [String] = []
        /// Waits until the native table shows exactly `rows` (indices into the fixture), under
        /// their original item IDs, and the status says `fragment`.
        func waitFor(_ fragment: String, rows: [Int]) async throws {
            let expected = rows.map { Self.fakeRows[$0] }
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "\(fragment) with rows \(rows)") {
                Self.nativeRows(table) == expected && statusField.stringValue.contains(fragment)
            }
            #expect(Self.modelItemIDs(applier) == rows.map { identities[$0] }, "\(fragment): no new identity")
            trace.append(statusField.stringValue)
        }

        // Only my processes: the fixture's own account owns PID 4101; the unreadable owner is
        // hidden and counted, never taken for root.
        NativeActivation.click(mine)
        try await waitFor("User: mine, UID 1000 (alice) · 1 of 3 processes match · 1 process with an unreadable owner hidden", rows: [0])
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "buttons follow the choice") {
            !mine.isEnabled && all.isEnabled
        }

        // A name typed into the user field: resolved on the server's own cache, then filtered.
        type("bob", into: userField, userAdapter)
        try await waitFor("User: name \"bob\", UID 1001 · 1 of 3 processes match", rows: [1])
        #expect(mine.isEnabled && all.isEnabled)

        // A UID no account names, which no process has: an explained empty result.
        type("4242", into: userField, userAdapter)
        try await waitFor("User: UID 4242 (no account name) · no process of 3 matches every filter: the user filter alone keeps 0", rows: [])
        #expect(table.numberOfRows == 0)

        // All visible users again: the button empties the user field.
        NativeActivation.click(all)
        try await waitFor("showing all 3 processes", rows: [0, 1, 2])
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the user field emptied") {
            userField.stringValue == "" && !all.isEnabled && mine.isEnabled
        }

        // A PID set with one PID no visible process has.
        type("4102 9999", into: pidSetField, pidSetAdapter)
        try await waitFor("PID set: 2 PIDs · no visible process has PID 9999 · 1 of 3 processes match", rows: [1])
        // Combined with the user filter: nothing matches both, and each filter's count is stated.
        NativeActivation.click(mine)
        try await waitFor("no process of 3 matches every filter: the user filter alone keeps 1, the PID set alone keeps 1", rows: [])

        // Cleared: every row back, every identity unchanged, every native handle the same.
        NativeActivation.click(all)
        type("", into: pidSetField, pidSetAdapter)
        try await waitFor("showing all 3 processes", rows: [0, 1, 2])
        #expect(Self.nativeRows(table) == Self.fakeRows)
        #expect(Self.modelItemIDs(applier) == identities)
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.ownerInputID)) === userHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.pidSetInputID)) === pidSetHandle)
        #expect(renderer.registry.handle(for: NodeId(Self.ownerStatusID)) === statusHandle)
        print("PX-012 native SSH evidence: window \(window.windowNumber); itemIDs=\(identities) unchanged=\(Self.modelItemIDs(applier) == identities); rows=\(Self.nativeRows(table)); statuses=\(trace); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-013: clicking a native row sends a standard `SELECTION_CHANGED`; the server resolves
    /// the row's item ID against the rows it publishes and describes that process instance in
    /// four standard status lines under the table. The native highlight stays the client's: the
    /// server selects nothing. Clicking A then B shows B; a sort that moves B's row keeps the
    /// inspector on B, because it follows the item ID, not the position; and a process whose
    /// details are denied says so.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func inspectorDescribesTheClickedProcessAndStaysOnItWhenASortMovesItsRow() async throws {
        let harness = try await Self.launch(arguments: ["--fake-source"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH with its inspector") {
            applier.lastAppliedRevision == Revision(1)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.inspectorMetricsID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let table = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let lineHandles = try Self.inspectorLineIDs.map { id in
            try #require(renderer.registry.handle(for: NodeId(id)), "inspector line \(id)")
        }
        let lines = try lineHandles.map { handle in try #require(handle.view as? NSTextField) }
        let sortByName = try #require(renderer.registry.handle(for: NodeId(Self.sortButtonIDs[1]))?.view as? NSButton)
        func native() -> [String] { lines.map(\.stringValue) }
        var trace: [[String]] = []
        func waitFor(_ expected: [String], _ step: String) async throws {
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: step) {
                native() == expected
            }
            #expect(Self.inspectorLines(applier) == expected, "\(step): the model holds what is drawn")
            trace.append(native())
        }

        // No selection.
        #expect(Self.nativeRows(table) == Self.fakeRows)
        #expect(table.selectedRowIndexes.isEmpty)
        try await waitFor(["Selected process: none · select a row to inspect it", "", "", ""], "no selection")

        // A: the first worker, owned by the fixture's own account.
        let workerA = [
            "Selected process: PID 4101 · worker",
            "Name: worker · PID 4101 · Owner: UID 1000 (alice)",
            "State: S (sleeping) · Parent: PID 1 · Started: 2027-01-15 06:20:00 UTC (server clock)",
            "CPU: 250.0% · Resident: 1.1 MiB",
        ]
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        try await waitFor(workerA, "A selected")
        let revisionAfterA = applier.lastAppliedRevision.value

        // Then B: the second worker, under another account; A never comes back.
        let workerB = [
            "Selected process: PID 4102 · worker",
            "Name: worker · PID 4102 · Owner: UID 1001 (bob)",
            "State: R (running) · Parent: PID 4101 · Started: 2027-01-15 07:43:20 UTC (server clock)",
            "CPU: 0.0% · Resident: 0 B",
        ]
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        try await waitFor(workerB, "B selected")
        #expect(applier.lastAppliedRevision.value > revisionAfterA)
        #expect(table.selectedRowIndexes == IndexSet(integer: 1), "the highlight is the client's own")

        // A sort by name moves B from row 1 to row 2; the highlight and the inspector follow B.
        let identities = Self.modelItemIDs(applier)
        NativeActivation.click(sortByName)
        let byName = [Self.fakeRows[2], Self.fakeRows[0], Self.fakeRows[1]]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by name") {
            Self.nativeRows(table) == byName && sortByName.title == "Sort by Name \u{2191}"
        }
        #expect(table.selectedRowIndexes == IndexSet(integer: 2), "the highlight followed B")
        #expect(Self.modelItemIDs(applier)[2] == identities[1], "B kept its item ID")
        #expect(native() == workerB, "the inspector stayed on B after its row moved")
        #expect(Self.inspectorLines(applier) == workerB)
        trace.append(native())

        // The helper, now on row 0: no PID, a denied owner and denied details, each said so.
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        try await waitFor([
            "Selected process: PID unavailable · helper · details: permission denied",
            "Name: helper · PID unavailable · Owner: permission denied",
            "State, parent and start time: permission denied",
            "CPU: Warming up · Resident: Denied",
        ], "denied details")

        // Every change happened inside the native controls built once.
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        for (handle, id) in zip(lineHandles, Self.inspectorLineIDs) {
            #expect(renderer.registry.handle(for: NodeId(id)) === handle, "inspector line \(id) was rebuilt")
        }
        print("PX-013 native SSH evidence: window \(window.windowNumber); itemIDs=\(identities) rowsByName=\(Self.nativeRows(table)); selectedRow=\(table.selectedRow); inspector=\(trace); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-013: a selected process that ends is reported as ended, with no details and no
    /// metrics, and the inspector stays on that instance: when the scripted fixture brings a
    /// process back under the same PID, it is a new row with a new item ID, and the inspector
    /// never shows it.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func inspectorReportsASelectedProcessThatEndedAndNeverItsPIDsNextRecord() async throws {
        let harness = try await Self.launch(arguments: ["--fake-sequence", "--refresh-interval-ms", "400"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the scripted explorer over SSH") {
            controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
                && renderer.registry.handle(for: NodeId(Self.inspectorMetricsID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let table = try #require((renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView)
        let lines = try Self.inspectorLineIDs.map { id in
            try #require(renderer.registry.handle(for: NodeId(id))?.view as? NSTextField)
        }
        func native() -> [String] { lines.map(\.stringValue) }
        func row4102() -> Int? { Self.nativeRows(table).firstIndex { $0.first == "4102" } }
        let selected = "Selected process: PID 4102 · worker"

        // Click PID 4102 while the script shows it. A click that lands after the script has
        // already ended it names an item the server no longer shows, which is refused and
        // explained; the test then waits for the next cycle and clicks again.
        var clickedItem: UInt64?
        for attempt in 1...6 {
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID 4102 shown (attempt \(attempt))") {
                row4102() != nil
            }
            let row = try #require(row4102())
            let item = Self.modelItemIDs(applier)[row]
            table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the click resolved") {
                native()[0].hasPrefix(selected) || native()[0].contains("is not a row this server shows")
            }
            if native()[0].hasPrefix(selected) {
                clickedItem = item
                break
            }
        }
        let item = try #require(clickedItem, "PID 4102 was selected while shown")

        // The script ends PID 4102: the inspector says so, and shows nothing of any other record.
        let ended = [
            "Selected process: PID 4102 · worker · this process has ended; select another row",
            "Name: worker · PID 4102 · Owner: unavailable",
            "State, parent and start time: none: the process has ended",
            "CPU and resident memory: none: the process has ended",
        ]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID 4102 ended") {
            native() == ended
        }
        #expect(Self.inspectorLines(applier) == ended)
        #expect(!Self.modelItemIDs(applier).contains(item))

        // The next cycle lists PID 4102 again under a new item ID: another instance as far as
        // identity goes, which the inspector does not adopt.
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID 4102 listed again") {
            row4102() != nil
        }
        let again = Self.modelItemIDs(applier)[try #require(row4102())]
        #expect(again != item, "a returning record is a new row")
        #expect(native() == ended, "the inspector never shows the PID's next record")
        print("PX-013 native SSH evidence (ended): window \(window.windowNumber); selected item \(item); returning item \(again); inspector=\(native()); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    /// PX-014: the selected process's command line is one standard read-only `RichText`, which
    /// the generic client renders as a non-editable, selectable `NSTextView`. A process whose
    /// arguments carry quotes, a newline, an ESC byte, shell metacharacters, a bidirectional
    /// override and a secret shows them as inert escaped text, one argument per line; the text
    /// carries no link and is copied by the native text system's own selection writer; another
    /// account's process shows its denied paths; and the server's own output never contains the
    /// secret.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func commandLineIsSelectableInertNativeTextAndNoSecretReachesTheServerLog() async throws {
        let log = FileManager.default.temporaryDirectory
            .appendingPathComponent("px014-srtop-\(UUID().uuidString).log")
        defer { try? FileManager.default.removeItem(at: log) }
        let harness = try await Self.launch(arguments: ["--fake-source"], serverLog: log)
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer with its command line") {
            applier.lastAppliedRevision == Revision(1)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(Self.inspectorCommandID)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let table = try #require((renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView)
        let commandHandle = try #require(renderer.registry.handle(for: NodeId(Self.inspectorCommandID)))
        let scroll = try #require(commandHandle.view as? NSScrollView, "a RichText is a scrolling text view")
        let textView = try #require(scroll.documentView as? NSTextView)
        // Normal native text: selectable, never editable, and nothing that turns text into a link
        // or an action.
        #expect(textView.isSelectable)
        #expect(!textView.isEditable)
        #expect(!textView.isAutomaticLinkDetectionEnabled)
        #expect(!textView.isAutomaticDataDetectionEnabled)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "no process selected") {
            textView.string == "Command line: no process selected"
        }

        // PID 4101: every tricky argument as inert text.
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID 4101's command line") {
            textView.string == Self.workerACommand
        }
        #expect(Self.modelText(applier, NodeId(Self.inspectorCommandID)) == Self.workerACommand)
        #expect(textView.isSelectable && !textView.isEditable, "an update keeps the view read-only")
        #expect(!textView.string.unicodeScalars.contains { [0x1b, 0x202e, 0x2028, 0x7f].contains($0.value) },
                "no control or bidirectional character reaches the native view")
        #expect(textView.string.components(separatedBy: "\n").count == 11, "one line per argument and per path")
        let storage = try #require(textView.textStorage)
        var linked = 0
        storage.enumerateAttribute(.link, in: NSRange(location: 0, length: storage.length)) { value, _, _ in
            if value != nil { linked += 1 }
        }
        #expect(linked == 0, "a server path or argument is never a link")
        window.contentView?.layoutSubtreeIfNeeded()
        #expect(scroll.frame.width > 0 && scroll.frame.height > 0, "the command line is laid out: \(scroll.frame)")
        // Copy uses the native text system: select everything and write the selection to a
        // private pasteboard (never the user's general one) through the writer Copy uses.
        textView.setSelectedRange(NSRange(location: 0, length: (textView.string as NSString).length))
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("org.srui.px014.\(UUID().uuidString)"))
        defer { pasteboard.releaseGlobally() }
        #expect(textView.selectedRange() == NSRange(location: 0, length: (textView.string as NSString).length))
        #expect(textView.writeSelection(to: pasteboard, types: textView.writablePasteboardTypes))
        #expect(pasteboard.string(forType: .string) == Self.workerACommand)
        let copiedWorkerA = pasteboard.string(forType: .string)?.count ?? 0

        // PID 4102, another account's process: its arguments, and its paths denied.
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "PID 4102's command line") {
            textView.string == Self.workerBCommand
        }
        #expect(!textView.string.contains("hunter2"), "nothing of the earlier target remains")
        // The helper: its whole record denied.
        table.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the helper's command line") {
            textView.string == "Command line: permission denied"
        }
        #expect(renderer.registry.handle(for: NodeId(Self.inspectorCommandID)) === commandHandle,
                "the command line was updated in place")
        print("PX-014 native SSH evidence: window \(window.windowNumber); view=\(type(of: textView)) selectable=\(textView.isSelectable) editable=\(textView.isEditable) linkDetection=\(textView.isAutomaticLinkDetectionEnabled) links=\(linked) frame=\(scroll.frame) copiedCharacters=\(copiedWorkerA) lines=\(Self.workerACommand.components(separatedBy: "\n").count); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
        let logged = try Data(contentsOf: log)
        #expect(!String(decoding: logged, as: UTF8.self).contains("hunter2-SECRET"), "the server never logs an argument")
        print("PX-014 native server log: \(logged.count) bytes; secret present: \(String(decoding: logged, as: UTF8.self).contains("hunter2-SECRET"))")
    }

    // PX-014: the inspector's command line and the fake fixture's views of it.
    fileprivate static let inspectorCommandID: UInt64 = 60
    fileprivate static let workerACommand = #"""
    Command line (argv on the server, shown as text, never run): 8 arguments, 182 bytes
    argv[0] · 33 bytes · /usr/libexec/srtop-fixture/worker
    argv[1] · 13 bytes · --role=worker
    argv[2] · 21 bytes · --label=it's "quoted"
    argv[3] · 17 bytes · line one\nline two
    argv[4] · 12 bytes · \x1b[31mred\x1b[0m
    argv[5] · 43 bytes · ; rm -rf / # $(touch /tmp/px014) `id` | tee
    argv[6] · 10 bytes · \u{202e}gpj.exe
    argv[7] · 25 bytes · --password=hunter2-SECRET
    Executable on the server: /usr/libexec/srtop-fixture/worker
    Working directory on the server: /home/alice/work
    """#
    fileprivate static let workerBCommand = #"""
    Command line (argv on the server, shown as text, never run): 2 arguments, 21 bytes
    argv[0] · 6 bytes · worker
    argv[1] · 13 bytes · --role=worker
    Executable on the server: permission denied
    Working directory on the server: permission denied
    """#

    // PX-013: the inspector's four lines.
    fileprivate static let inspectorMetricsID: UInt64 = 59
    fileprivate static let inspectorLineIDs: [UInt64] = [56, 57, 58, 59]

    /// The inspector lines the client holds (PX-013).
    @MainActor
    fileprivate static func inspectorLines(_ applier: TransactionApplier) -> [String] {
        inspectorLineIDs.map { modelText(applier, NodeId($0)) }
    }

    // PX-012: the user-filter controls.
    fileprivate static let ownerMineID: UInt64 = 50
    fileprivate static let ownerAllID: UInt64 = 51
    fileprivate static let ownerInputID: UInt64 = 52
    fileprivate static let pidSetInputID: UInt64 = 53
    fileprivate static let ownerStatusID: UInt64 = 54

    // PX-011: the search controls.
    fileprivate static let searchInputID: UInt64 = 43
    fileprivate static let searchPreviousID: UInt64 = 44
    fileprivate static let searchNextID: UInt64 = 45
    fileprivate static let pidInputID: UInt64 = 46
    fileprivate static let pidGoID: UInt64 = 47
    fileprivate static let searchStatusID: UInt64 = 48

    /// PX-068: the column chooser is a set of standard native buttons; a click is a standard
    /// `ACTIVATE`, the server republishes the table's `columns` and every row's cells in the
    /// chosen sequence, and the generic client shows them in the same native table with the
    /// rows still in their sorted order and the native selection still on the same process,
    /// even while the sorted column is hidden.
    @Test(.timeLimit(.minutes(1)))
    @MainActor
    func columnChooserHidesReordersAndRestoresColumnsAndTheSelectionStaysOnItsProcess() async throws {
        let harness = try await Self.launch(arguments: ["--fake-source"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH") {
            applier.lastAppliedRevision == Revision(1)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: NodeId(5)) != nil
        }
        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)
        #expect(table.tableColumns.map(\.title) == Self.defaultHeadings)
        #expect(Self.nativeRows(table) == Self.fakeRows)
        let identities = Self.modelItemIDs(applier)
        #expect(identities.count == 3)

        // Sort by name (ascending): helper, then the two workers in identity order.
        let sortByName = try #require(renderer.registry.handle(for: NodeId(Self.sortButtonIDs[1]))?.view as? NSButton)
        NativeActivation.click(sortByName)
        let byName = [Self.fakeRows[2], Self.fakeRows[0], Self.fakeRows[1]]
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the order by name") {
            Self.nativeRows(table) == byName
        }
        let sortedIDs = [identities[2], identities[0], identities[1]]
        #expect(Self.modelItemIDs(applier) == sortedIDs)

        // Select the second worker natively, as a user would (row 2 of the sorted table).
        table.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        let selected = identities[1]
        #expect(table.selectedRowIndexes == IndexSet(integer: 2))

        /// The sorted rows projected onto `columns`, indices into the default layout.
        func projected(_ columns: [Int]) -> [[String]] {
            byName.map { row in columns.map { row[$0] } }
        }
        func headings(_ columns: [Int]) -> [String] {
            columns.map { Self.defaultHeadings[$0] }
        }
        func expectSelectionKept(_ step: String) {
            #expect(Self.modelItemIDs(applier) == sortedIDs, "\(step): every process kept its item ID and its sorted place")
            #expect(table.selectedRowIndexes == IndexSet(integer: 2), "\(step): the native selection stayed on its process")
            #expect(table.selectedRow >= 0 && Self.modelItemIDs(applier)[table.selectedRow] == selected, "\(step)")
        }

        // Verify the chooser row has a horizontal scrolling container so all buttons
        // remain accessible at ordinary window widths (PX-068):
        let chooserRowHandle = try #require(renderer.registry.handle(for: NodeId(28)))
        let chooserScroll = try #require(chooserRowHandle.view.enclosingScrollView, "chooser row is inside a scrolling container")
        #expect(chooserScroll.hasHorizontalScroller)

        // Trailing buttons like CPU and Restore defaults are inside the scrolling container
        // and can be scrolled into view:
        let initialHideCPU = try Self.chooserButton(renderer, column: 3, verbs: ["hide"])
        #expect(initialHideCPU.enclosingScrollView === chooserScroll)
        #expect(initialHideCPU.window === window)
        initialHideCPU.scrollToVisible(initialHideCPU.bounds)

        let restoreHandle = try #require(renderer.registry.handle(for: NodeId(41)))
        let restoreButton = try #require(restoreHandle.view as? NSButton)
        #expect(restoreButton.enclosingScrollView === chooserScroll)
        #expect(restoreButton.window === window)
        restoreButton.scrollToVisible(restoreButton.bounds)

        // Hide the sorted column, Name: its heading and its cells go; the rows keep the name order.
        let hideName = try Self.chooserButton(renderer, column: 1, verbs: ["hide"])
        NativeActivation.click(hideName)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "Name hidden") {
            table.tableColumns.map(\.title) == headings([0, 2, 3]) && Self.nativeRows(table) == projected([0, 2, 3])
        }
        #expect(Self.modelRows(applier) == projected([0, 2, 3]))
        expectSelectionKept("hiding the sorted column")

        // Move Resident earlier: it goes ahead of PID, and the cells follow their headings.
        let moveResident = try Self.chooserButton(renderer, column: 2, verbs: ["left", "earlier"])
        NativeActivation.click(moveResident)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "Resident moved earlier") {
            table.tableColumns.map(\.title) == headings([2, 0, 3]) && Self.nativeRows(table) == projected([2, 0, 3])
        }
        #expect(Self.modelRows(applier) == projected([2, 0, 3]))
        expectSelectionKept("reordering the columns")

        // Hide CPU too.
        let hideCPU = try Self.chooserButton(renderer, column: 3, verbs: ["hide"])
        NativeActivation.click(hideCPU)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "CPU hidden") {
            table.tableColumns.map(\.title) == headings([2, 0]) && Self.nativeRows(table) == projected([2, 0])
        }
        expectSelectionKept("hiding a second column")

        // Restore the defaults: every column returns in catalogue order, the rows still by name.
        let restore = try Self.restoreDefaultsButton(renderer)
        NativeActivation.click(restore)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "default columns restored") {
            table.tableColumns.map(\.title) == Self.defaultHeadings && Self.nativeRows(table) == byName
        }
        #expect(Self.modelRows(applier) == byName)
        expectSelectionKept("restoring the defaults")

        // Every change happened inside the native table built once.
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(scroll.documentView as? NSTableView === table)
        print("PX-068 native SSH evidence: window \(window.windowNumber); columns=\(table.tableColumns.map(\.title)); rows=\(Self.nativeRows(table)); itemIDs=\(Self.modelItemIDs(applier)); selected item \(selected) at row \(table.selectedRow); revision=\(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    // PX-068: the catalogued headings, and the short names the chooser's labels may use instead.
    private static let defaultHeadings = ["PID", "Name", "Resident", "CPU (100% = 1 CPU)"]
    private static let columnButtonNames = ["PID", "Name", "Resident", "CPU"]

    /// Every enabled native button outside the sort row, with its title.
    @MainActor
    private static func enabledNonSortButtons(_ renderer: AppKitRenderer) -> [NSButton] {
        renderer.registry.allHandles.compactMap { $0.view as? NSButton }.filter { button in
            button.isEnabled && !button.title.lowercased().contains("sort")
        }
    }

    /// The one enabled chooser button for the default layout's `column` whose title states one
    /// of `verbs`, found by what a user reads on it, as the Rust suite does.
    @MainActor
    private static func chooserButton(_ renderer: AppKitRenderer, column: Int, verbs: [String]) throws -> NSButton {
        let all = enabledNonSortButtons(renderer)
        let matching = all.filter { button in
            let title = button.title.lowercased()
            return (title.contains(columnButtonNames[column].lowercased()) || title.contains(defaultHeadings[column].lowercased()))
                && verbs.contains { title.contains($0) }
        }
        try #require(matching.count == 1, "expected one column chooser button for \(defaultHeadings[column]) labelled \(verbs); buttons: \(all.map(\.title))")
        return matching[0]
    }

    /// The one enabled chooser button that restores the default columns.
    @MainActor
    private static func restoreDefaultsButton(_ renderer: AppKitRenderer) throws -> NSButton {
        let all = enabledNonSortButtons(renderer)
        let matching = all.filter { button in
            let title = button.title.lowercased()
            return title.contains("default") && (title.contains("restore") || title.contains("reset"))
        }
        try #require(matching.count == 1, "expected one button restoring the default columns; buttons: \(all.map(\.title))")
        return matching[0]
    }

    /// PX-010-G01: 1,000 synthetic rows over real SSH with continuous churn.
    /// Asserts native macOS AppKit rendering performance, memory and layout stability,
    /// and selection preservation under active row churn.
    @Test(.timeLimit(.minutes(2)))
    @MainActor
    func largeCollectionRendersNativelyAndPreservesSelectionUnderChurn() async throws {
        let harness = try await Self.launch(arguments: ["--scale-source", "1000", "--scale-interval-ms", "500"])
        defer { Self.shutDown(harness) }
        _ = NSApplication.shared
        let (controller, applier, renderer) = Self.genericClient(SSHTransport(configuration: harness.configuration))
        try await controller.start()

        try await AsyncTestSupport.eventually(timeout: .seconds(25), description: "1,000 synthetic rows over SSH") {
            applier.lastAppliedRevision.value >= 1
                && renderer.registry.handle(for: NodeId(5)) != nil
                && (renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView.map { ($0 as? NSTableView)?.numberOfRows == 1000 } == true
        }

        let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
        defer { window.orderOut(nil); window.close() }
        let tableHandle = try #require(renderer.registry.handle(for: NodeId(5)))
        let scroll = try #require(tableHandle.view as? NSScrollView)
        let table = try #require(scroll.documentView as? NSTableView)

        #expect(table.numberOfRows == 1000)
        #expect(table.tableColumns.map(\.title) == ["PID", "Name", "Resident", "CPU (100% = 1 CPU)"])

        // Visible cells in the initial viewport render native strings properly
        for col in 0..<4 {
            let cell = try #require(table.view(atColumn: col, row: 0, makeIfNecessary: true) as? NSTextField)
            #expect(!cell.stringValue.isEmpty)
        }

        // Select row 50 (a persistent daemon/service process that does not churn)
        let targetIndex = 50
        table.selectRowIndexes(IndexSet(integer: targetIndex), byExtendingSelection: false)
        let initialIdentities = Self.modelItemIDs(applier)
        #expect(initialIdentities.count == 1000)
        let selectedID = initialIdentities[targetIndex]
        #expect(table.selectedRowIndexes == IndexSet(integer: targetIndex))
        #expect(table.selectedRow == targetIndex)

        // Wait for multiple churn cycles to execute on server (at 500ms intervals)
        let revBefore = applier.lastAppliedRevision
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "multiple churn cycles committed") {
            applier.lastAppliedRevision.value >= revBefore.value + 4
        }

        // Under continuous churn, total rows remain capped at 1,000
        #expect(table.numberOfRows == 1000)
        let currentIdentities = Self.modelItemIDs(applier)
        #expect(currentIdentities.count == 1000)

        // Selection is natively preserved anchored to ItemId across deletions and insertions
        #expect(currentIdentities.contains(selectedID))
        let newRowIndex = try #require(currentIdentities.firstIndex(of: selectedID))
        #expect(table.selectedRowIndexes == IndexSet(integer: newRowIndex))
        #expect(table.selectedRow == newRowIndex)

        // Verify native table handles and window remain intact
        #expect(renderer.registry.handle(for: NodeId(5)) === tableHandle)
        #expect(window.isVisible)

        print("PX-010-G01 native AppKit scale evidence: window \(window.windowNumber); 1000 rows rendered natively; selected item \(selectedID) preserved at row \(table.selectedRow) across churn to revision \(applier.lastAppliedRevision.value)")
        await controller.stop()
    }

    // PX-010: the filter row, input and status nodes.
    fileprivate static let filterRowID: UInt64 = 25
    fileprivate static let filterInputID: UInt64 = 26
    fileprivate static let filterStatusID: UInt64 = 27

    // PX-009: the sort row's four buttons, in column order.
    fileprivate static let sortButtonIDs: [UInt64] = [21, 22, 23, 24]
    private static let fakeRows = [
        ["4101", "worker", "1.1 MiB", "250.0%"],
        ["4102", "worker", "0 B", "0.0%"],
        ["Unavailable", "helper", "Denied", "Warming up"],
    ]

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
        binary: String = "apps/srtop/target/debug/srtop", arguments: [String], serverLog: URL? = nil
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
            // A test process must never inherit the test binary's stdio. PX-014 keeps the
            // server's own output in a file it then reads, to show what the server logged.
            if let serverLog {
                guard FileManager.default.createFile(atPath: serverLog.path, contents: nil) else {
                    throw CocoaError(.fileWriteUnknown)
                }
                let logHandle = try FileHandle(forWritingTo: serverLog)
                server.standardOutput = logHandle
                server.standardError = logHandle
            } else {
                server.standardOutput = FileHandle.nullDevice
                server.standardError = FileHandle.nullDevice
            }
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
