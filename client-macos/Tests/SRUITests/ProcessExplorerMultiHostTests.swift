//
// ProcessExplorerMultiHostTests.swift
// SRUITests
//
// PX-095: two hosts at once through one generic connection manager (§6.2, §6.3, §17, §18;
// D2 T23, T24, T38). Each host is its own localhost sshd, `srui-ssh-bridge` and `srtop
// --fake-source --actions dry-run`, so the two serve the same node IDs, the same PIDs and the same
// process names by construction, and nothing in the client knows which application it is. The
// test drives both sessions natively and checks that a selection, an action, a confirmation, an
// event sequence and a cached identity stay in their own session, that each window names its
// host, account and incarnation, and that replacing one session leaves the other running.
//

import AppKit
import Darwin
import Foundation
@testable import ConnectionManager
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import TransportSSH

/// The session context each port's attempts run in, as the manager hands it to the factory.
@MainActor
final class MultiHostContexts {
    var byPort: [UInt16: ConnectionSessionContext] = [:]
}

/// One host's session as the test reads it: everything is looked up afresh, because the generic
/// renderer remounts on every structural commit (§23).
@MainActor
struct MultiHostSide {
    let name: String
    let port: UInt16
    let contexts: MultiHostContexts

    var context: ConnectionSessionContext? { contexts.byPort[port] }
    var registry: RenderRegistry? { context?.renderer.registry }
    func window() -> NSWindow? { registry?.handle(for: NodeId(1))?.window }
    func button(_ id: UInt64) -> NSButton? { registry?.handle(for: NodeId(id))?.view as? NSButton }
    func text(_ id: UInt64) -> String { (registry?.handle(for: NodeId(id))?.view as? NSTextField)?.stringValue ?? "" }
    func table() -> NSTableView? { (registry?.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView }
    func rows() -> [[String]] { table().map(ProcessExplorerShellTests.nativeRows) ?? [] }
    func selectedPIDs() -> [String] {
        guard let table = table() else { return [] }
        let shown = rows()
        return table.selectedRowIndexes.compactMap { $0 < shown.count ? shown[$0][0] : nil }
    }
    func inspector() -> String { text(ProcessExplorerShellTests.inspectorLineIDs[0]) }
    func status() -> String { text(ProcessExplorerShellTests.actionsStatusID) }
    func confirm(_ slot: UInt64) -> NSButton? {
        registry.flatMap { ProcessExplorerShellTests.panelConfirm($0, slot) }
    }
    func confirmNode(_ slot: UInt64) -> NodeId? {
        registry.flatMap { ProcessExplorerShellTests.panelHandle($0, slot: slot, offset: 2)?.nodeID }
    }
    func itemIDs() -> [UInt64] { context.map { ProcessExplorerShellTests.modelItemIDs($0.applier) } ?? [] }
    func revision() -> UInt64 { context?.applier.lastAppliedRevision.value ?? 0 }
    func eventSeq() async -> UInt64 { await context?.outbox.eventSeq ?? 0 }
    func select(_ pid: String) throws {
        let row = try #require(rows().firstIndex { $0.first == pid }, "\(name) shows PID \(pid)")
        try #require(table()).selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
    }
    func describe() -> String {
        "\(name): revision=\(revision()) selected=\(selectedPIDs()) inspector=\"\(inspector())\" "
            + "status=\"\(status())\" confirm0=\(confirmNode(0).map { "\($0.value)" } ?? "none") "
            + "identity=\"\(window()?.sessionIdentityText ?? "<none>")\" title=\"\(window()?.title ?? "")\""
    }
}

extension ProcessExplorerShellTests {
    @Test(.timeLimit(.minutes(4)))
    @MainActor
    func twoHostsWithIdenticalProcessesKeepSelectionActionsAndIdentityApart() async throws {
        let arguments = ["--fake-source", "--actions", "dry-run", "--refresh-interval-ms", "300000"]
        var hostA = try await Self.launch(arguments: arguments)
        defer { Self.shutDown(hostA) }
        let hostB = try await Self.launch(arguments: arguments)
        defer { Self.shutDown(hostB) }
        _ = NSApplication.shared
        let portA = try #require(hostA.configuration.port), portB = try #require(hostB.configuration.port)
        let configurations = [portA: hostA.configuration, portB: hostB.configuration]
        let contexts = MultiHostContexts()
        let manager = ConnectionManager(
            store: SavedConnectionStore(url: hostA.temp.appendingPathComponent("connections.json")),
            attemptFactory: { request in
                // The manager's own SSH posture, aimed at this fixture's sshd and keys.
                let port = request.connection.port ?? 0
                contexts.byPort[port] = request.context
                return ConnectionManager.makeSessionControllerAttempt(
                    request: request,
                    transport: SSHTransport(configuration: configurations[port] ?? hostA.configuration)
                )
            })
        var managerShutDown = false
        defer {
            for context in contexts.byPort.values {
                for handle in context.renderer.registry.surfaceHandles {
                    handle.window?.orderOut(nil)
                    handle.window?.close()
                }
            }
            if !managerShutDown { Task { @MainActor in await manager.shutdown() } }
        }
        let a = MultiHostSide(name: "A", port: portA, contexts: contexts)
        let b = MultiHostSide(name: "B", port: portB, contexts: contexts)
        var evidence: [String] = []
        func record(_ phase: String) {
            evidence.append("\(phase): \(a.describe()) | \(b.describe())")
        }
        func waitFor(_ description: String, _ condition: @MainActor () -> Bool) async throws {
            do {
                try await AsyncTestSupport.eventually(timeout: .seconds(30), description: description, condition: condition)
            } catch {
                record("TIMED OUT \(description)")
                for line in evidence { print("PX-095 native evidence: \(line)") }
                throw error
            }
        }
        let user = NSUserName()
        let idA = try #require(manager.connect(ConnectDraft(label: "srtop", host: "127.0.0.1", port: String(portA), user: user)))
        let idB = try #require(manager.connect(ConnectDraft(label: "srtop", host: "127.0.0.1", port: String(portB), user: user)))
        func session(_ id: SavedConnection.ID) -> String? { manager.entries.first { $0.id == id }?.sessionID }
        try await waitFor("both hosts connected with the fixture rows") {
            manager.status(for: idA) == .connected && manager.status(for: idB) == .connected
                && a.rows() == Self.fakeRows && b.rows() == Self.fakeRows
        }
        #expect(manager.open(id: idA) && manager.open(id: idB))
        let windowA = try #require(a.window()), windowB = try #require(b.window())
        SurfacePresentation.forHostApplication().present(windowA)
        SurfacePresentation.forHostApplication().present(windowB)

        // Identical by construction: the same title, rows, PIDs, names and item identities, so
        // nothing a server sends tells the two apart.
        let sessionA = try #require(session(idA)), sessionB = try #require(session(idB))
        #expect(sessionA != sessionB, "two incarnations")
        #expect(windowA !== windowB)
        #expect(windowA.title == "Process Explorer" && windowB.title == "Process Explorer")
        #expect(a.itemIDs() == b.itemIDs() && !a.itemIDs().isEmpty,
                "the same ItemIds on both hosts: a cache keyed by them alone would collide")
        // Only the client's own chrome does: host, account and incarnation on every window.
        #expect(windowA.sessionIdentityText == "\(user)@127.0.0.1:\(portA) · session \(sessionA) · saved as “srtop”")
        #expect(windowB.sessionIdentityText == "\(user)@127.0.0.1:\(portB) · session \(sessionB) · saved as “srtop”")
        #expect(windowA.tabbingIdentifier != windowB.tabbingIdentifier)
        let contextA = try #require(a.context), contextB = try #require(b.context)
        #expect(contextA !== contextB && contextA.outbox !== contextB.outbox)
        #expect(contextA.outbox.clientInstanceId != contextB.outbox.clientInstanceId)
        record("connected")

        // A selection belongs to one session: selecting 4101 on A selects nothing on B, and B's
        // own selection of 4102 leaves A's alone.
        let seqB0 = await b.eventSeq()
        try a.select("4101")
        try await waitFor("A's inspector on 4101") { a.inspector() == "Selected process: PID 4101 · worker" }
        try await Task.sleep(for: .milliseconds(300))
        #expect(b.selectedPIDs().isEmpty && b.inspector() == "Selected process: none · select a row to inspect it")
        #expect(await b.eventSeq() == seqB0, "A's selection allocated nothing in B's outbox")
        // Verification round 1 (W3): B rebuilds every view while A holds its local selection, once
        // by a remount (as a resynchronization's snapshot does) and once by a warm reconnect. A
        // renderer restores a collection's selection from its own memory on every remount, so a
        // memory shared between sessions would put A's 4101 (the same ItemId) into B's table.
        let remountedB = try #require(b.context)
        try remountedB.renderer.attach(store: remountedB.applier.store)
        try await Task.sleep(for: .milliseconds(300))
        #expect(b.selectedPIDs().isEmpty && b.inspector() == "Selected process: none · select a row to inspect it",
                "B's remount kept B's own selection: \(b.describe())")
        manager.connect(id: idB)
        try await waitFor("B warm-reconnected") {
            manager.status(for: idB) == .connected && b.rows() == Self.fakeRows
        }
        try await Task.sleep(for: .milliseconds(300))
        #expect(b.selectedPIDs().isEmpty && b.inspector() == "Selected process: none · select a row to inspect it",
                "B's warm reconnect kept B's own selection: \(b.describe())")
        #expect(session(idB) == sessionB, "the same incarnation, resumed")
        #expect(a.selectedPIDs() == ["4101"])
        record("B remounted and reconnected while A held 4101")
        let seqA1 = await a.eventSeq()
        try b.select("4102")
        try await waitFor("B's inspector on 4102") { b.inspector() == "Selected process: PID 4102 · worker" }
        try await Task.sleep(for: .milliseconds(300))
        #expect(a.selectedPIDs() == ["4101"] && a.inspector() == "Selected process: PID 4101 · worker")
        #expect(await a.eventSeq() == seqA1, "B's selection allocated nothing in A's outbox")
        // And the other way round: A remounts while B holds 4102; A keeps 4101.
        let remountedA = try #require(a.context)
        try remountedA.renderer.attach(store: remountedA.applier.store)
        try await Task.sleep(for: .milliseconds(300))
        #expect(a.selectedPIDs() == ["4101"] && a.inspector() == "Selected process: PID 4101 · worker",
                "A's remount kept A's own selection: \(a.describe())")
        #expect(b.selectedPIDs() == ["4102"])
        record("selected apart")

        // An action and its confirmation belong to one session. A prepares 4101: A's window shows
        // the panel, B's does not.
        NativeActivation.click(try #require(a.button(Self.actionPrepareID)))
        try await waitFor("A prepared") {
            a.status().hasPrefix("Prepared #1: terminate PID 4101 · worker (dry run)") && a.confirm(0)?.isEnabled == true
        }
        #expect(b.confirm(0) == nil && b.status().hasPrefix("Dry run: Prepare checks"), "\(b.describe())")
        let confirmA = try #require(a.confirm(0)), confirmNodeA = try #require(a.confirmNode(0))
        // Visibly bound: the Confirm is in A's window, which names A's host, account and session.
        #expect(confirmA.window === a.window())
        #expect(a.window()?.sessionIdentityText?.hasPrefix("\(user)@127.0.0.1:\(portA) · session \(sessionA)") == true)
        #expect(confirmA.title == "Confirm dry run: terminate PID 4101 · worker")

        // B moves onto 4101 too and prepares its own: the same PID, name and panel slot on both.
        // B's selection change does not withdraw A's intent.
        try b.select("4101")
        try await waitFor("B's inspector on 4101") { b.inspector() == "Selected process: PID 4101 · worker" }
        NativeActivation.click(try #require(b.button(Self.actionPrepareID)))
        try await waitFor("B prepared") {
            b.status().hasPrefix("Prepared #1: terminate PID 4101 · worker (dry run)") && b.confirm(0)?.isEnabled == true
        }
        let confirmNodeB = try #require(b.confirmNode(0))
        #expect(confirmNodeA != confirmNodeB, "each srtop run draws its own panel range (PX-031)")
        #expect(a.status().hasPrefix("Prepared #1: terminate PID 4101 · worker (dry run)") && a.confirm(0)?.isEnabled == true,
                "B's selection and prepare left A's intent pending: \(a.describe())")
        #expect(try #require(b.confirm(0)).window === b.window() && b.window() !== a.window())
        record("both prepared")

        // A's confirmation cannot be moved onto B: delivered to B's session, it names a node B's
        // replica does not hold, so B's controller refuses it before anything is allocated or sent,
        // and B's own intent stays pending.
        let seqB2 = await b.eventSeq(), revisionB2 = b.revision()
        contextB.renderer.onInteraction?(.activate(nodeID: confirmNodeA))
        try await Task.sleep(for: .milliseconds(500))
        #expect(await b.eventSeq() == seqB2, "nothing was allocated in B's outbox")
        #expect(b.revision() == revisionB2 && b.status().hasPrefix("Prepared #1") && b.confirm(0)?.isEnabled == true, "\(b.describe())")
        #expect(a.status().hasPrefix("Prepared #1"), "\(a.describe())")

        // A confirms natively: A's request is recorded; B's stays pending.
        let seqB3 = await b.eventSeq()
        NativeActivation.click(confirmA)
        try await waitFor("A's dry run requested") {
            a.status().hasPrefix("Dry run #1 requested (terminate PID 4101 · worker): nothing was sent") && a.confirm(0) == nil
        }
        try await Task.sleep(for: .milliseconds(300))
        #expect(b.status().hasPrefix("Prepared #1: terminate PID 4101 · worker (dry run)") && b.confirm(0)?.isEnabled == true, "\(b.describe())")
        #expect(await b.eventSeq() == seqB3, "A's confirmation used A's outbox only")
        record("A confirmed")

        // Replacing A's session (its daemon restarts: a new incarnation on the same endpoint)
        // leaves B's session running: its status, window, replica, selection and intent.
        let windowBBefore = try #require(b.window()), revisionBBefore = b.revision()
        var bStatuses: [ConnectionStatus] = []
        func bStillConnected() -> Bool {
            let status = manager.status(for: idB)
            if bStatuses.last != status { bStatuses.append(status) }
            return true
        }
        kill(hostA.server.processIdentifier, SIGTERM)
        hostA.server.waitUntilExit()
        try await waitFor("A disconnected by its restart") {
            bStillConnected() && manager.status(for: idA) == .disconnected(resumeAvailable: true)
        }
        hostA.server = try await Self.relaunchServer(in: hostA, arguments: arguments)
        manager.connect(id: idA)
        try await waitFor("A replaced and rebuilt") {
            bStillConnected() && manager.status(for: idA) == .connected && session(idA) != sessionA
                && a.rows() == Self.fakeRows && a.status().hasPrefix("Dry run: Prepare checks")
        }
        let replacementA = try #require(session(idA))
        #expect(manager.alert?.kind == .sessionReplaced && manager.alert?.connectionID == idA)
        #expect(manager.alert?.message.contains("on \(user)@127.0.0.1:\(portA) was replaced by \(replacementA)") == true,
                "\(manager.alert?.message ?? "no alert")")
        manager.dismissAlert()
        #expect(a.window()?.sessionIdentityText == "\(user)@127.0.0.1:\(portA) · session \(replacementA) · saved as “srtop”")
        #expect(a.selectedPIDs().isEmpty && a.confirm(0) == nil, "nothing of A's old incarnation survives")
        #expect(bStatuses == [.connected], "B never left connected: \(bStatuses)")
        #expect(session(idB) == sessionB && b.window() === windowBBefore && windowBBefore.isVisible)
        #expect(windowBBefore.subtitle.isEmpty && windowBBefore.sessionIdentityText?.contains("session \(sessionB)") == true)
        #expect(b.revision() >= revisionBBefore)
        #expect(b.selectedPIDs() == ["4101"] && b.inspector() == "Selected process: PID 4101 · worker")
        #expect(b.status().hasPrefix("Prepared #1: terminate PID 4101 · worker (dry run)") && b.confirm(0)?.isEnabled == true)
        #expect(a.itemIDs() == b.itemIDs(), "the new incarnation serves the same ItemIds again")
        record("A replaced")

        // B confirms its own intent natively, after all of that; A shows nothing of it.
        NativeActivation.click(try #require(b.confirm(0)))
        try await waitFor("B's dry run requested") {
            b.status().hasPrefix("Dry run #1 requested (terminate PID 4101 · worker): nothing was sent") && b.confirm(0) == nil
        }
        #expect(a.status().hasPrefix("Dry run: Prepare checks") && a.confirm(0) == nil, "\(a.describe())")
        record("B confirmed")

        await manager.shutdown()
        managerShutDown = true
        for line in evidence { print("PX-095 native evidence: \(line)") }
    }
}
