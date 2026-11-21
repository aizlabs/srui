//
// ProcessExplorerAutomationTests.swift
// SRUITests
//
// PX-026: the Process Explorer driven through the in-process T35 semantic inspection and
// automation API (D1 §22.9, §7.7, §18.2; D2 T24, T35). Every test runs the real srtop server
// over a real localhost SSH subsystem into the unchanged generic client, and
//
//  - locates controls by semantic identity, role, label or text, never by an NSView, a screen
//    coordinate or a process-specific client path;
//  - invokes only harmless view actions (Pause view/Resume view, Refresh now, a row selection);
//    srtop has no process-changing action and none is added;
//  - proves at the wire that an automated action is the same `Event` frame, on the same logical
//    channel, in the same contiguous `event_seq` space and with the same `ServerEventAck`
//    settlement as a native click on the same control, and that a disabled or unsupported control
//    refuses automation exactly as it refuses a native click: the same refusal and no frame;
//  - writes a developer-only semantic inspection report (IDs, roles, labels, values, state and
//    advertised events/actions) that names no AppKit type, and checks with lsof that inspecting
//    and automating opens no listener in this process. T35 stays in-process: no XPC, TCP or
//    unix-socket automation endpoint exists (§22.9 "SHOULD be disabled ... by default").
//

import Accessibility
import AppKit
import Darwin
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import TransportSSH

extension ProcessExplorerShellTests {
    // Node identities the server publishes. Tests locate every control semantically and only then
    // check that the located node is the one expected.
    fileprivate static let automationPauseID = NodeId(62)
    fileprivate static let automationRefreshID = NodeId(63)
    fileprivate static let automationPauseLabel = "Pause view"
    fileprivate static let automationResumeLabel = "Resume view"
    fileprivate static let automationRefreshLabel = "Refresh now"

    /// PX-026 acceptance 1, 2 and 5: Pause view and Resume view located through T35 by role and
    /// label while the native button is hidden and untitled, so neither a view lookup nor a
    /// coordinate could find it. Native, automated, automated, native: four `ACTIVATE` frames
    /// with one shape, on the same logical channel, with contiguous `event_seq`, each settled by a
    /// PROCESSED `ServerEventAck` of one shape, and the outbox left empty. Inspecting and
    /// automating opens no listener in this process.
    @Test(.timeLimit(.minutes(2)))
    @MainActor
    func automationLocatesControlsSemanticallyAndTakesTheNativeClicksWireAndSettlementPath() async throws {
        try await Self.withAutomationExplorer(arguments: ["--fake-sequence", "--refresh-interval-ms", "300"]) { client in
            try await client.waitUntilReady(Self.automationPauseID, Self.automationRefreshID)
            let endpointsBefore = try await AutomationEndpoints.listing()
            let inspector = client.controller.makeSemanticInspector()
            client.inspector = inspector

            // Locate the control semantically; only the native comparison touches its view.
            let pause = try client.locate(role: .button, label: Self.automationPauseLabel)
            #expect(pause.id == Self.automationPauseID)
            #expect(pause.supportedEventTypes == [.EVENT_ACTIVATE])
            #expect(pause.node.state.enabled)
            let pauseButton = try #require(client.renderer.registry.view(for: pause.id) as? NSButton)

            var origins: [String] = []
            var nativeRevisions: [Int: ClosedRange<UInt64>] = [:]
            var automatedEvents: [Int: (handle: SemanticNodeHandle, event: Event)] = [:]
            var retention: [String] = []

            // Every action runs with the client's inbound stream held: once its frame is on the
            // wire and srtop's PROCESSED ack has reached the tap, but before the client applies
            // it, the outbox must still retain exactly that event (its seq allocated, nothing
            // acknowledged past the one before), so a reconnect could replay it (§18.2). The same
            // check applies to native and automated actions; releasing the stream settles it.
            @MainActor func whileSettlementIsHeld(_ origin: String, _ act: @MainActor () async throws -> Void) async throws {
                let seq = await client.outbox.eventSeq + 1
                client.tap.holdInbound()
                try await act()
                try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "\(origin) event \(seq) sent and acknowledged on the wire, ack held") {
                    client.tap.events.contains { $0.wire.eventSeq == seq } && client.tap.acknowledgement(settling: seq) != nil
                }
                let pending = await client.outbox.pendingCount
                let acked = await client.outbox.lastAckedEventSeq
                let allocated = await client.outbox.eventSeq
                #expect(pending == 1, "\(origin) event \(seq) is retained until its ack is applied")
                #expect(acked == seq - 1, "\(origin) event \(seq) is not settled before its ack is applied")
                #expect(allocated == seq)
                retention.append("\(origin) seq \(seq): pending=\(pending) lastAcked=\(acked) while held")
                client.tap.releaseInbound()
                try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(15), description: "\(origin) event \(seq) settled once its ack is applied") {
                    let pending = await client.outbox.pendingCount
                    let acked = await client.outbox.lastAckedEventSeq
                    return pending == 0 && acked == seq
                }
            }
            @MainActor func clickNatively(expecting label: String) async throws {
                try await whileSettlementIsHeld("native") {
                    let low = client.applier.currentSnapshot.revision.value
                    NativeActivation.click(pauseButton)
                    let high = client.applier.currentSnapshot.revision.value
                    nativeRevisions[origins.count] = low...high
                }
                origins.append("native")
                try await client.waitForLabel(Self.automationPauseID, label)
            }
            @MainActor func activateSemantically(_ current: String, expecting label: String) async throws {
                let restore = client.concealNativeView(of: Self.automationPauseID)
                let handle = try client.locate(role: .button, label: current)
                #expect(handle.id == Self.automationPauseID, "\(current) is the same node")
                try await whileSettlementIsHeld("automation") {
                    let event = try await handle.activate()
                    automatedEvents[origins.count] = (handle, event)
                }
                restore()
                origins.append("automation")
                try await client.waitForLabel(Self.automationPauseID, label)
            }

            try await clickNatively(expecting: Self.automationResumeLabel)
            try await activateSemantically(Self.automationResumeLabel, expecting: Self.automationPauseLabel)
            try await activateSemantically(Self.automationPauseLabel, expecting: Self.automationResumeLabel)
            try await clickNatively(expecting: Self.automationPauseLabel)
            #expect(pauseButton.title == Self.automationPauseLabel, "the native button shows the server's label")

            let settled = try await client.waitForSettlement(of: 4)
            let events = settled.map(\.event)
            let acks = settled.map(\.ack)
            #expect(origins == ["native", "automation", "automation", "native"])

            // One frame shape: every field but the per-event identity and observed revision.
            let eventShapes = events.map { AutomationWireTap.shape(of: $0.wire) }
            #expect(eventShapes.allSatisfy { $0 == eventShapes[0] }, "one Event shape: \(eventShapes)")
            #expect(events.allSatisfy { $0.wire.nodeID == Self.automationPauseID.value })
            #expect(events.allSatisfy { $0.wire.eventType == AutomationWireTap.wireType(.EVENT_ACTIVATE) })
            #expect(events.allSatisfy { $0.wire.arguments.isEmpty && $0.wire.editSeq == 0 })
            #expect(events.allSatisfy { $0.logicalClass == events[0].logicalClass })
            #expect(events.allSatisfy { !$0.wire.clientInstanceID.isEmpty && !$0.wire.eventID.isEmpty })
            #expect(Set(events.map(\.wire.eventID)).count == 4, "every event has its own event_id")
            let firstSeq = events[0].wire.eventSeq
            #expect(events.map(\.wire.eventSeq) == Array(firstSeq..<(firstSeq + 4)),
                    "native and automated events share one contiguous event_seq space")
            for (index, event) in events.enumerated() {
                if let range = nativeRevisions[index] {
                    #expect(range.contains(event.wire.observedRevision), "native observed_revision \(event.wire.observedRevision) in \(range)")
                }
                if let automated = automatedEvents[index] {
                    #expect(event.wire.observedRevision == automated.handle.observedRevision.value)
                    #expect(event.wire.eventSeq == automated.event.eventSeq)
                    #expect(event.wire.eventID == automated.event.eventId.bytes)
                }
            }

            // One settlement shape: PROCESSED, echoed identity, the session that produced it.
            let ackShapes = acks.map { AutomationWireTap.shape(of: $0) }
            #expect(ackShapes.allSatisfy { $0 == ackShapes[0] }, "one ServerEventAck shape: \(ackShapes)")
            for (event, ack) in zip(events, acks) {
                #expect(ack.status == .processed, "event \(event.wire.eventSeq) settled \(ack.status)")
                #expect(ack.eventID == event.wire.eventID)
                #expect(ack.clientInstanceID == event.wire.clientInstanceID)
                #expect(ack.revisionAfterEffect >= event.wire.observedRevision)
                #expect(ack.sessionID == client.controller.sessionId)
            }
            #expect(await client.outbox.pendingCount == 0)
            #expect(await client.outbox.lastAckedEventSeq == firstSeq + 3)

            // The developer-only report: semantic identity, values and advertised actions only.
            let tree = inspector.snapshot()
            let report = SemanticInspectionReport.render(tree)
            #expect(report.contains("#62 Button label=\"Pause view\" events=[ACTIVATE]"))
            #expect(report.contains("#5 Table label=\"Processes\" "))
            #expect(report.contains("events=[SELECTION_CHANGED]"))
            try client.expectNoNativeType(in: report, tree: tree)
            try SemanticInspectionReport.write(report, name: "px026-full-tree")

            // Inspecting and automating opened no listener in this process. A control listener of
            // each kind, opened and closed here, shows the listing would have seen one.
            let endpointsAfter = try await AutomationEndpoints.listing()
            try await AutomationEndpoints.expectListingDetectsAControlListener()
            #expect(endpointsAfter.listeners.subtracting(endpointsBefore.listeners).isEmpty,
                    "new listeners: \(endpointsAfter.listeners.subtracting(endpointsBefore.listeners))")
            print("PX-026 wire evidence: origins=\(origins) seqs=\(events.map(\.wire.eventSeq)) observed=\(events.map(\.wire.observedRevision)) class=\(events.map { "\($0.logicalClass)" }) acks=\(acks.map { "\($0.settledEventSeq):\($0.status)@\($0.revisionAfterEffect)" }) session=\(client.controller.sessionId ?? "-")")
            print("PX-026 retention evidence: \(retention)")
            print("PX-026 endpoint evidence: listeners before=\(endpointsBefore.listeners.sorted()) after=\(endpointsAfter.listeners.sorted()); sockets before=\(endpointsBefore.socketCount) after=\(endpointsAfter.socketCount)")
            try SemanticInspectionReport.write(
                "# lsof -nP -a -p \(getpid()) -i / -U before inspection\n\(endpointsBefore.raw)\n# after\n\(endpointsAfter.raw)",
                name: "px026-lsof"
            )
        }
    }

    /// PX-026 acceptance 3: a disabled control refuses automation exactly as it refuses a native
    /// click, and so does a control that advertises no such event. Refresh now is disabled while
    /// the view is paused: AppKit sends no action, the same interaction delivered to the client is
    /// refused by the same check that refuses automation (`nodeDisabled`), and neither puts a
    /// frame on the wire or consumes an `event_seq`. A Text line activated natively and
    /// semantically reports the same `unsupportedAction`. The next allowed action takes the very
    /// next `event_seq`.
    @Test(.timeLimit(.minutes(2)))
    @MainActor
    func aDisabledOrUnsupportedControlRefusesAutomationExactlyAsItRefusesANativeClick() async throws {
        try await Self.withAutomationExplorer(arguments: ["--fake-sequence", "--refresh-interval-ms", "300"]) { client in
            try await client.waitUntilReady(Self.automationPauseID, Self.automationRefreshID)
            let inspector = client.controller.makeSemanticInspector()
            client.inspector = inspector
            let nativeErrors = AutomationErrorRecorder()
            client.controller.nativeSemanticActionErrorReportedForTesting = { nativeErrors.record($0) }

            let pauseButton = try #require(client.renderer.registry.view(for: Self.automationPauseID) as? NSButton)
            let refreshButton = try #require(client.renderer.registry.view(for: Self.automationRefreshID) as? NSButton)
            NativeActivation.click(pauseButton)
            try await client.waitForLabel(Self.automationPauseID, Self.automationResumeLabel)
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "Refresh now disabled while paused") {
                inspector.snapshot().node(Self.automationRefreshID)?.state.enabled == false && !refreshButton.isEnabled
            }
            let paused = try await client.waitForSettlement(of: 1)
            let seqBefore = await client.outbox.eventSeq
            #expect(seqBefore == paused[0].event.wire.eventSeq)

            // Automation: located semantically, disabled in the snapshot, refused by the client.
            let refresh = try client.locate(role: .button, label: Self.automationRefreshLabel)
            #expect(refresh.id == Self.automationRefreshID)
            #expect(!refresh.node.state.enabled)
            await #expect(throws: SemanticAutomationError.nodeDisabled(Self.automationRefreshID)) {
                try await refresh.activate()
            }
            // A native click: AppKit sends no action for a disabled control.
            NativeActivation.click(refreshButton)
            // The same interaction delivered to the client (a click on a stale native control)
            // reaches the same check, which refuses it silently: a disabled control is a race,
            // not a contract failure, so nothing is reported either way.
            client.renderer.onInteraction?(.activate(nodeID: Self.automationRefreshID))

            // A Text line advertises no ACTIVATE: the same unsupportedAction both ways.
            let tree = inspector.snapshot()
            let line = try #require(tree.nodes.first { $0.role == .text && $0.text?.hasPrefix("Selected process:") == true },
                                    "the inspector's first line, located by its text")
            #expect(line.supportedEventTypes.isEmpty)
            let lineHandle = try #require(inspector.handle(for: line.id, in: tree))
            var automatedError: SemanticAutomationError?
            do {
                try await lineHandle.activate()
            } catch let error as SemanticAutomationError {
                automatedError = error
            }
            #expect(automatedError == .unsupportedAction(nodeID: line.id, eventType: .EVENT_ACTIVATE))
            client.renderer.onInteraction?(.activate(nodeID: line.id))
            try await AsyncTestSupport.eventually(timeout: .seconds(5), description: "the native refusal reported") {
                !nativeErrors.errors().isEmpty
            }
            #expect(nativeErrors.errors() == [automatedError].compactMap { $0 },
                    "the native refusal is the automated one, and the disabled click reported nothing")

            // Nothing reached the wire, no event_seq was consumed, the server stayed paused.
            try await Task.sleep(for: .milliseconds(600))
            #expect(client.tap.events.count == 1, "refusals sent no frame: \(client.tap.events.map(\.wire.nodeID))")
            #expect(await client.outbox.eventSeq == seqBefore)
            #expect(await client.outbox.pendingCount == 0)
            #expect(inspector.snapshot().node(Self.automationPauseID)?.label == Self.automationResumeLabel)

            // The next allowed action takes the very next event_seq; Refresh now comes back.
            let resume = try client.locate(role: .button, label: Self.automationResumeLabel)
            let resumed = try await resume.activate()
            #expect(resumed.eventSeq == seqBefore + 1)
            try await client.waitForLabel(Self.automationPauseID, Self.automationPauseLabel)
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "Refresh now enabled again") {
                inspector.snapshot().node(Self.automationRefreshID)?.state.enabled == true && refreshButton.isEnabled
            }
            let enabledRefresh = try client.locate(role: .button, label: Self.automationRefreshLabel)
            let refreshed = try await enabledRefresh.activate()
            #expect(refreshed.eventSeq == seqBefore + 2)
            let settled = try await client.waitForSettlement(of: 3)
            #expect(settled.map(\.event.wire.nodeID) == [Self.automationPauseID.value, Self.automationPauseID.value, Self.automationRefreshID.value])
            #expect(settled.allSatisfy { $0.ack.status == .processed })
            print("PX-026 refusal evidence: disabled=\(SemanticAutomationError.nodeDisabled(Self.automationRefreshID)) unsupported=\(automatedError.map { "\($0)" } ?? "-") nativeReported=\(nativeErrors.errors()) frames=\(settled.map { "\($0.event.wire.eventSeq):#\($0.event.wire.nodeID):\($0.ack.status)" })")
        }
    }

    /// PX-026 acceptance 4 and 5: select a process row semantically and read the inspector's
    /// semantic content. The row's ItemId comes from the semantic model the table node binds; the
    /// selection is a `SELECTION_CHANGED` of the same shape as a native row selection; the
    /// inspector, found by its text and hierarchy, describes the selected process; the subtree
    /// report names no AppKit type and no object reachable from the snapshot is an NSObject.
    @Test(.timeLimit(.minutes(2)))
    @MainActor
    func automationSelectsAProcessRowAndReadsTheInspectorSemantically() async throws {
        try await Self.withAutomationExplorer(arguments: ["--fake-source"]) { client in
            try await client.waitUntilReady(NodeId(5))
            let inspector = client.controller.makeSemanticInspector()
            client.inspector = inspector

            let tables = inspector.findAll(role: .table)
            try #require(tables.count == 1, "one table: \(tables.map(\.id))")
            let table = tables[0]
            #expect(table.supportedEventTypes.contains(.EVENT_SELECTION_CHANGED))
            // The rows the table binds, from the semantic replica (never from the native table).
            let rows = try client.semanticRows(of: table.id)
            let workerA = try #require(rows.first { $0.cells.first == "4101" }, "PID 4101: \(rows)")
            let workerB = try #require(rows.first { $0.cells.first == "4102" }, "PID 4102: \(rows)")

            @MainActor func inspectorLines() -> [String] {
                let tree = inspector.snapshot()
                guard let first = tree.nodes.first(where: { $0.role == .text && $0.text?.hasPrefix("Selected process:") == true }),
                      let parent = first.parentID.flatMap(tree.node) else { return [] }
                return parent.childIDs.compactMap { tree.node($0)?.text }
            }
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "no selection yet") {
                inspectorLines().first == "Selected process: none · select a row to inspect it"
            }

            // A native selection of A, then a semantic selection of B.
            let nativeTable = try #require((client.renderer.registry.view(for: table.id) as? NSScrollView)?.documentView as? NSTableView)
            let rowA = try #require(rows.firstIndex { $0.itemID == workerA.itemID })
            nativeTable.selectRowIndexes(IndexSet(integer: rowA), byExtendingSelection: false)
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "A inspected") {
                inspectorLines().first == "Selected process: PID 4101 · worker"
            }
            let fresh = try #require(inspector.findAll(role: .table).first)
            let selected = try await fresh.select(workerB.itemID)
            let expectedB = [
                "Selected process: PID 4102 · worker",
                "Name: worker · PID 4102 · Owner: UID 1001 (bob)",
                "State: R (running) · Parent: PID 4101 · Started: 2027-01-15 07:43:20 UTC (server clock)",
                "CPU: 0.0% · Resident: 0 B",
            ]
            try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "B inspected") {
                Array(inspectorLines().prefix(4)) == expectedB
            }

            let settled = try await client.waitForSettlement(of: 2)
            let native = settled[0].event.wire, automated = settled[1].event.wire
            #expect(AutomationWireTap.shape(of: native) == AutomationWireTap.shape(of: automated), "one SELECTION_CHANGED shape")
            #expect(native.eventType == AutomationWireTap.wireType(.EVENT_SELECTION_CHANGED))
            #expect(native.nodeID == table.id.value && automated.nodeID == table.id.value)
            #expect(native.arguments.map(\.property) == automated.arguments.map(\.property))
            #expect(native.arguments != automated.arguments, "each names its own item")
            #expect(automated.eventSeq == native.eventSeq + 1)
            #expect(automated.eventSeq == selected.eventSeq && automated.observedRevision == fresh.observedRevision.value)
            #expect(settled[0].event.logicalClass == settled[1].event.logicalClass)
            #expect(AutomationWireTap.shape(of: settled[0].ack) == AutomationWireTap.shape(of: settled[1].ack))
            #expect(settled.allSatisfy { $0.ack.status == .processed })

            // The developer-only report of the inspector's subtree after the semantic selection.
            let tree = inspector.snapshot()
            let line = try #require(tree.nodes.first { $0.text == expectedB[0] })
            let parentID = try #require(line.parentID)
            let report = SemanticInspectionReport.render(tree, under: parentID)
            #expect(report.contains("text=\"Selected process: PID 4102 · worker\""))
            try client.expectNoNativeType(in: report, tree: tree)
            try SemanticInspectionReport.write(report, name: "px026-inspector-after-selection")
            print(report)
            print("PX-026 selection evidence: table #\(table.id.value) item A=\(workerA.itemID.value) (native) item B=\(workerB.itemID.value) (automation) seqs=\(settled.map(\.event.wire.eventSeq)) observed=\(settled.map(\.event.wire.observedRevision)) acks=\(settled.map { "\($0.ack.status)" }) inspector=\(inspectorLines())")
        }
    }

    /// Starts srtop over SSH, a generic client with a wire tap and its own outbox, runs `body`,
    /// then stops the client and ends every fixture process on a dedicated thread, never on the
    /// main actor or the cooperative pool.
    @MainActor
    fileprivate static func withAutomationExplorer(
        arguments: [String],
        _ body: @MainActor (AutomationClient) async throws -> Void
    ) async throws {
        let harness = try await Self.launch(arguments: arguments)
        _ = NSApplication.shared
        let client = AutomationClient(transport: SSHTransport(configuration: harness.configuration))
        var failure: (any Error)?
        do {
            try await client.controller.start()
            try await body(client)
        } catch {
            failure = error
        }
        client.tap.releaseInbound()
        await client.controller.stop()
        client.closeWindows()
        let fixture = AutomationUnchecked(harness)
        await onAutomationThread { Self.shutDown(fixture.value) }
        if let failure { throw failure }
    }
}

/// The unchanged generic client (`genericClient`'s wiring), keeping its outbox and wire tap.
@MainActor
fileprivate final class AutomationClient {
    let tap: AutomationWireTap
    let outbox = EventOutbox()
    let applier = TransactionApplier()
    let renderer = AppKitRenderer()
    let controller: SessionController
    /// The T35 inspector a test made; `locate` only ever reads it.
    var inspector: SemanticInspector?

    init(transport: any Transport) {
        tap = AutomationWireTap(inner: transport)
        controller = SessionController(transport: tap, applier: applier, outbox: outbox, renderer: renderer)
        controller.attachRenderer(renderer)
    }

    func waitUntilReady(_ nodes: NodeId...) async throws {
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "the explorer over SSH") {
            self.applier.lastAppliedRevision.value >= 1
                && self.controller.isEventDispatchEnabled
                && nodes.allSatisfy { self.renderer.registry.handle(for: $0) != nil }
        }
    }

    /// Exactly one node of `role` labelled `label` in one fresh semantic snapshot.
    func locate(role: TypeRef, label: String) throws -> SemanticNodeHandle {
        let inspector = try #require(self.inspector, "make the inspector first")
        let matches = inspector.findAll(role: role, label: label)
        try #require(matches.count == 1, "one \(role) labelled \(label): \(matches.map(\.id))")
        return matches[0]
    }

    func waitForLabel(_ id: NodeId, _ label: String) async throws {
        let inspector = try #require(self.inspector)
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "#\(id.value) labelled \(label)") {
            inspector.snapshot().node(id)?.label == label
        }
    }

    /// Hides the node's native view and blanks its title, so only the semantic tree can still
    /// find the control; returns the undo. The renderer may retitle it meanwhile (a new label).
    func concealNativeView(of id: NodeId) -> () -> Void {
        guard let button = renderer.registry.view(for: id) as? NSButton else { return {} }
        let hidden = button.isHidden, title = button.title
        button.isHidden = true
        button.title = ""
        return {
            button.isHidden = hidden
            if button.title.isEmpty { button.title = title }
        }
    }

    /// The first `count` events this client sent, each with the acknowledgement that settled it.
    func waitForSettlement(of count: Int) async throws -> [(event: AutomationWireTap.SentEvent, ack: SRUIServerEventAck)] {
        try await AsyncTestSupport.eventually(timeout: .seconds(15), description: "\(count) events sent and settled") {
            let events = self.tap.events
            return events.count >= count
                && events.prefix(count).allSatisfy { self.tap.acknowledgement(settling: $0.wire.eventSeq) != nil }
        }
        return try tap.events.prefix(count).map { event in
            (event, try #require(tap.acknowledgement(settling: event.wire.eventSeq)))
        }
    }

    /// The rows the table binds, from the semantic replica: item ID and cell text in model order.
    func semanticRows(of table: NodeId) throws -> [(itemID: ItemId, cells: [String])] {
        let model = try #require(applier.currentSnapshot.store.getModelForNode(table), "#\(table.value) binds a model")
        return model.items.sorted { $0.key < $1.key }.map { entry in
            guard case .list(let cells) = entry.value.value else { return (entry.value.itemID, []) }
            return (entry.value.itemID, cells.map { cell in
                switch cell {
                case .unsignedInt(let number): return String(number)
                case .string(let text): return text
                default: return String(describing: cell)
                }
            })
        }
    }

    /// Neither the report nor anything reachable from the snapshot names or holds an AppKit object.
    func expectNoNativeType(in report: String, tree: SemanticTreeSnapshot) throws {
        var nativeNames = Set(renderer.registry.allHandles.map { NSStringFromClass(type(of: $0.view)) })
        nativeNames.formUnion(renderer.registry.allHandles.compactMap { $0.window.map { NSStringFromClass(type(of: $0)) } })
        try #require(nativeNames.contains("NSButton") && nativeNames.contains("NSScrollView"), "native classes: \(nativeNames)")
        let named = nativeNames.filter { report.contains($0) }
        #expect(named.isEmpty, "the report names native classes \(named)")
        #expect(report.range(of: #"\bNS[A-Z][A-Za-z]+"#, options: .regularExpression) == nil, "an NS* type in the report")
        #expect(report.range(of: #"0x[0-9a-fA-F]{6,}"#, options: .regularExpression) == nil, "a pointer in the report")
        let reachable = try SemanticInspectionReport.reachableObjects(from: tree)
        #expect(reachable.nsObjects.isEmpty, "NSObjects reachable from the snapshot: \(reachable.nsObjects)")
    }

    func closeWindows() {
        for handle in renderer.registry.allHandles {
            handle.window?.orderOut(nil)
            handle.window?.close()
        }
    }
}

/// The developer-only semantic inspection output (PX-026 acceptance 5): one line per node with
/// its semantic ID, role, label, text, value, non-default state, advertised standard events and
/// server-advertised action names. It reads only the T35 snapshot, so it cannot name a view.
enum SemanticInspectionReport {
    static func render(_ tree: SemanticTreeSnapshot, under root: NodeId? = nil) -> String {
        var nodes = tree.nodes
        if let root {
            var subtree: [SemanticNodeSnapshot] = []
            func visit(_ id: NodeId) {
                guard let node = tree.node(id) else { return }
                subtree.append(node)
                node.childIDs.forEach(visit)
            }
            visit(root)
            nodes = subtree
        }
        let baseDepth = nodes.first?.depth ?? 1
        var lines = ["# Semantic inspection (developer-only test output, D1 §22.9): epoch \(tree.epoch.rawValue), "
            + "revision \(tree.revision.value), \(nodes.count) of \(tree.nodes.count) nodes"]
        for node in nodes {
            lines.append(String(repeating: "  ", count: max(0, node.depth - baseDepth)) + line(node))
        }
        return lines.joined(separator: "\n") + "\n"
    }

    static func line(_ node: SemanticNodeSnapshot) -> String {
        var parts = ["#\(node.id.value)", node.role.standardName ?? node.role.description]
        if let label = node.label { parts.append("label=\(quote(label))") }
        if let text = node.text { parts.append("text=\(quote(text))") }
        if let value = node.value { parts.append("value=\(quote(value.description))") }
        if let description = node.accessibleDescription { parts.append("description=\(quote(description))") }
        if !node.state.enabled { parts.append("disabled") }
        if node.state.selected { parts.append("selected") }
        if node.state.busy { parts.append("busy") }
        if node.state.readOnly { parts.append("read-only") }
        if let visibility = node.state.visibility, visibility != .visibilityVisible {
            parts.append("visibility=\(visibility.standardValueName ?? visibility.description)")
        }
        if !node.supportedEventTypes.isEmpty {
            parts.append("events=[" + node.supportedEventTypes.map { $0.standardEventName ?? $0.description }.joined(separator: ",") + "]")
        }
        if !node.actions.isEmpty { parts.append("actions=[" + node.actions.map(quote).joined(separator: ",") + "]") }
        if let actionKey = node.actionKey { parts.append("action_key=\(quote(actionKey))") }
        if let shortcut = node.shortcut { parts.append("shortcut=\(shortcut)") }
        if !node.childIDs.isEmpty { parts.append("children=\(node.childIDs.count)") }
        return parts.joined(separator: " ")
    }

    static func quote(_ text: String) -> String {
        "\"" + text.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n") + "\""
    }

    /// Every class instance reachable from `root` by reflection, and which of them are NSObjects.
    /// The walk is bounded; reaching the bound fails rather than passing on a partial walk.
    static func reachableObjects(from root: Any, limit: Int = 500_000) throws -> (classes: Set<String>, nsObjects: [String]) {
        var classes = Set<String>(), nsObjects: [String] = []
        var stack: [Any] = [root]
        var visited = 0
        while let value = stack.popLast() {
            visited += 1
            try #require(visited <= limit, "the reflection walk passed \(limit) values")
            let dynamicType = type(of: value)
            if let objectType = dynamicType as? AnyClass {
                classes.insert(String(reflecting: objectType))
                if objectType is NSObject.Type { nsObjects.append(NSStringFromClass(objectType)) }
            }
            stack.append(contentsOf: Mirror(reflecting: value).children.map(\.value))
        }
        return (classes, nsObjects)
    }

    /// Writes the report where `PX026_EVIDENCE_DIR` names a directory.
    static func write(_ text: String, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["PX026_EVIDENCE_DIR"] else { return }
        let url = URL(fileURLWithPath: directory)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        try text.write(to: url.appendingPathComponent("\(name).txt"), atomically: true, encoding: .utf8)
    }
}

/// A pass-through transport that also decodes every `Event` the client sends, with its logical
/// class, and every `ServerEventAck` it receives. Chunks and acknowledgements are forwarded
/// unchanged, so flow control stays the inner transport's own.
fileprivate final class AutomationWireTap: Transport, @unchecked Sendable {
    struct SentEvent: Sendable {
        let wire: SRUIEvent
        let logicalClass: LogicalChannelClass
    }

    private let inner: any Transport
    private let lock = NSLock()
    private var sent: [SentEvent] = []
    private var acks: [SRUIServerEventAck] = []
    private var pending = Data()
    private var holding = false
    private var held: [Data] = []
    private var heldContinuation: AsyncThrowingStream<Data, Error>.Continuation?

    init(inner: any Transport) {
        self.inner = inner
    }

    var events: [SentEvent] { lock.withLock { sent } }

    func acknowledgement(settling seq: UInt64) -> SRUIServerEventAck? {
        lock.withLock { acks.first { $0.settledEventSeq == seq } }
    }

    /// The frame without its per-event identity (`event_seq`, `event_id`) and observed revision.
    static func shape(of event: SRUIEvent) -> SRUIEvent {
        var shape = event
        shape.eventSeq = 0
        shape.eventID = Data()
        shape.observedRevision = 0
        shape.arguments = shape.arguments.map { argument in
            var property = argument
            property.clearValue()
            return property
        }
        return shape
    }

    /// The acknowledgement without the identity and revision of the event it settles.
    static func shape(of ack: SRUIServerEventAck) -> SRUIServerEventAck {
        var shape = ack
        shape.eventID = Data()
        shape.lastProcessedEventSeq = 0
        shape.settledEventSeq = 0
        shape.revisionAfterEffect = 0
        return shape
    }

    static func wireType(_ type: TypeRef) -> Srui_Protocol_TypeRef {
        var wire = Srui_Protocol_TypeRef()
        wire.namespaceID = type.namespaceID
        wire.localID = type.localID
        return wire
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        try await inner.send(data: data, logicalClass: logicalClass)
        guard let message = try? decodeFramedMessage(from: data), case .event(let wire) = message.msg else { return }
        lock.withLock { sent.append(SentEvent(wire: wire, logicalClass: logicalClass)) }
    }

    func receiveStream() -> AsyncThrowingStream<Data, Error> {
        let inner = self.inner
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await chunk in inner.receiveStream() {
                        self.observe(chunk)
                        self.deliver(chunk, to: continuation)
                    }
                    self.releaseInbound()
                    continuation.finish()
                } catch {
                    self.releaseInbound()
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    func close() async {
        await inner.close()
    }

    func acknowledgeReceived(byteCount: Int) async {
        await inner.acknowledgeReceived(byteCount: byteCount)
    }

    /// Holds every inbound chunk from the client while still decoding it here, so a test can see a
    /// `ServerEventAck` on the wire before the client has applied it. `releaseInbound` delivers the
    /// held chunks in order; the stream's end also releases them, and so does fixture teardown.
    func holdInbound() {
        lock.withLock { holding = true }
    }

    func releaseInbound() {
        lock.withLock {
            holding = false
            if let continuation = heldContinuation {
                for chunk in held { continuation.yield(chunk) }
            }
            held.removeAll()
        }
    }

    private func deliver(_ chunk: Data, to continuation: AsyncThrowingStream<Data, Error>.Continuation) {
        lock.withLock {
            heldContinuation = continuation
            if holding {
                held.append(chunk)
            } else {
                continuation.yield(chunk)
            }
        }
    }

    private func observe(_ chunk: Data) {
        lock.lock()
        defer { lock.unlock() }
        pending.append(chunk)
        while let prefix = Self.lengthPrefix(pending), pending.count >= prefix.header + prefix.length {
            let base = pending.startIndex
            let payload = pending.subdata(in: (base + prefix.header)..<(base + prefix.header + prefix.length))
            pending = pending.subdata(in: (base + prefix.header + prefix.length)..<pending.endIndex)
            if let message = try? SRUIMessage(serializedBytes: payload), case .serverEventAck(let ack) = message.msg {
                acks.append(ack)
            }
        }
    }

    private static func lengthPrefix(_ data: Data) -> (length: Int, header: Int)? {
        var value = 0
        for (index, byte) in data.prefix(10).enumerated() {
            value |= Int(byte & 0x7f) << (7 * index)
            if byte & 0x80 == 0 { return (value, index + 1) }
        }
        return nil
    }
}

/// This test process's own sockets as lsof reports them, and which of them listen: a TCP socket
/// in LISTEN, a bound UDP socket, or a unix socket bound to a path rather than connected (`->`).
/// lsof's field output (`-F`) is parsed, one record per file descriptor, so no column can shift.
fileprivate enum AutomationEndpoints {
    struct Listing: Sendable {
        let listeners: Set<String>
        let sockets: [String]
        var socketCount: Int { sockets.count }
        var raw: String { sockets.joined(separator: "\n") }
    }

    static func listing() async throws -> Listing {
        let pid = getpid()
        let network = try await lsof(["-nP", "-T", "s", "-F", "ftPnT", "-a", "-p", "\(pid)", "-i"])
        let unix = try await lsof(["-nP", "-F", "ftn", "-a", "-p", "\(pid)", "-U"])
        var records: [[Character: [String]]] = []
        for line in (network + "\n" + unix).split(separator: "\n") {
            guard let tag = line.first else { continue }
            let value = String(line.dropFirst())
            if tag == "p" { continue }
            if tag == "f" { records.append([:]) }
            guard !records.isEmpty else { continue }
            records[records.count - 1][tag, default: []].append(value)
        }
        var listeners = Set<String>(), sockets: [String] = []
        for record in records {
            let fd = record["f"]?.first ?? "?", type = record["t"]?.first ?? ""
            let proto = record["P"]?.first ?? "", name = record["n"]?.first ?? ""
            let states = record["T"] ?? []
            let summary = "\(fd) \(type) \(proto) \(name) \(states.joined(separator: ","))"
            sockets.append(summary)
            let listens = states.contains("ST=LISTEN") || proto == "UDP" || (type == "unix" && name.hasPrefix("/"))
            if listens { listeners.insert(summary) }
        }
        return Listing(listeners: listeners, sockets: sockets)
    }
    /// Positive control: a TCP listener on 127.0.0.1 and a unix listener this test opens, lists
    /// and closes at once, so an empty listing is a finding, not a blind lsof or parser. Test code
    /// only; nothing in the client listens.
    static func expectListingDetectsAControlListener() async throws {
        let tcp = socket(AF_INET, SOCK_STREAM, 0)
        try #require(tcp >= 0)
        defer { Darwin.close(tcp) }
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        address.sin_port = 0
        let tcpBound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(tcp, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
        }
        try #require(tcpBound == 0 && listen(tcp, 1) == 0, "control TCP listener: errno \(errno)")

        let path = "/tmp/px026-\(getpid())-\(UInt32.random(in: 0...UInt32.max)).sock"
        let unix = socket(AF_UNIX, SOCK_STREAM, 0)
        try #require(unix >= 0)
        defer {
            Darwin.close(unix)
            unlink(path)
        }
        var unixAddress = sockaddr_un()
        unixAddress.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &unixAddress.sun_path) { buffer in
            for (index, byte) in path.utf8.enumerated() where index < buffer.count - 1 { buffer[index] = byte }
        }
        unixAddress.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let unixBound = withUnsafePointer(to: &unixAddress) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(unix, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        try #require(unixBound == 0 && listen(unix, 1) == 0, "control unix listener: errno \(errno)")

        let control = try await listing()
        print("PX-026 lsof positive control: \(control.listeners.sorted())")
        #expect(control.listeners.contains { $0.contains("TCP 127.0.0.1:") && $0.contains("ST=LISTEN") },
                "the control TCP listener is listed: \(control.raw)")
        #expect(control.listeners.contains { $0.contains(path) }, "the control unix listener is listed: \(control.raw)")
    }

    /// Runs lsof with its output in a file (no pipe to fill) and waits through its termination
    /// handler, never `waitUntilExit()`. Exit 1 only means nothing matched.
    private static func lsof(_ arguments: [String]) async throws -> String {
        let output = FileManager.default.temporaryDirectory
            .appendingPathComponent("px026-lsof-\(UUID().uuidString).txt")
        guard FileManager.default.createFile(atPath: output.path, contents: nil) else {
            throw CocoaError(.fileWriteUnknown)
        }
        defer { try? FileManager.default.removeItem(at: output) }
        let handle = try FileHandle(forWritingTo: output)
        defer { try? handle.close() }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = handle
        process.standardError = FileHandle.nullDevice
        let status: Int32 = try await withCheckedThrowingContinuation { continuation in
            process.terminationHandler = { continuation.resume(returning: $0.terminationStatus) }
            do {
                try process.run()
            } catch {
                process.terminationHandler = nil
                continuation.resume(throwing: error)
            }
        }
        try #require(status == 0 || status == 1, "lsof \(arguments) exited \(status)")
        return try String(contentsOf: output, encoding: .utf8)
    }
}

fileprivate final class AutomationErrorRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recorded: [SemanticAutomationError] = []

    func record(_ error: SemanticAutomationError) {
        lock.withLock { recorded.append(error) }
    }

    func errors() -> [SemanticAutomationError] {
        lock.withLock { recorded }
    }
}

fileprivate struct AutomationUnchecked<Value>: @unchecked Sendable {
    let value: Value
    init(_ value: Value) { self.value = value }
}

/// Runs `body` on a dedicated thread: fixture teardown waits for processes, which must never
/// block the main actor or a cooperative-pool thread.
fileprivate func onAutomationThread(_ body: @escaping @Sendable () -> Void) async {
    await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
        Thread {
            body()
            continuation.resume()
        }.start()
    }
}
