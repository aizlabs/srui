// Semantic automation integration coverage (§7.7, §22.9, §32.11).

import Accessibility
import AppKit
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import Text
import TransportSSH

@Suite("Semantic inspection automation")
struct SemanticInspectionAutomationTests {
    @Test("Native and inspected activation use the same semantic input path")
    @MainActor
    func nativeAndInspectedActivationProduceEquivalentEvents() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let handle = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )
            let button = try #require(
                harness.renderer.registry.view(for: SemanticInspectionFixture.approveID)
                    as? NSButton
            )

            button.performClick(nil)
            let returnedEvent = try await handle.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.count == 2)
            let native = try #require(captured.first)
            let inspected = try #require(captured.last)

            #expect(native.logicalClass == .input)
            #expect(inspected.logicalClass == .input)
            #expect(native.event.clientInstanceId == inspected.event.clientInstanceId)
            #expect(native.event.observedRevision == inspected.event.observedRevision)
            #expect(native.event.observedRevision == Revision(1))
            #expect(native.event.nodeId == inspected.event.nodeId)
            #expect(native.event.nodeId == SemanticInspectionFixture.approveID)
            #expect(native.event.eventType == inspected.event.eventType)
            #expect(native.event.eventType == .EVENT_ACTIVATE)
            #expect(native.event.arguments == inspected.event.arguments)
            #expect(native.event.editSeq == inspected.event.editSeq)
            #expect(native.event.eventSeq == 1)
            #expect(inspected.event.eventSeq == 2)
            #expect(native.event.eventId != inspected.event.eventId)
            #expect(returnedEvent == inspected.event)
        }
    }

    @Test("A handle rechecks enabled state and emits no event after disablement")
    @MainActor
    func disabledHandleIsRejectedWithoutAnOutboundEvent() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let handle = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )

            try await harness.disableApproveButton()
            let button = try #require(
                harness.renderer.registry.view(for: SemanticInspectionFixture.approveID)
                    as? NSButton
            )
            #expect(button.isEnabled == false)
            button.performClick(nil)

            do {
                _ = try await handle.activate()
                Issue.record("Expected disabled semantic node activation to fail")
            } catch SemanticAutomationError.nodeDisabled(let nodeID) {
                #expect(nodeID == SemanticInspectionFixture.approveID)
            } catch {
                Issue.record("Expected nodeDisabled, got \(error)")
            }

            let sentinel = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.sentinelLabel)
            )
            _ = try await sentinel.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.count == 1)
            #expect(captured.first?.event.nodeId == SemanticInspectionFixture.sentinelID)
            #expect(
                captured.contains {
                    $0.event.nodeId == SemanticInspectionFixture.approveID
                } == false
            )
        }
    }

    @Test("A handle retained across replacement cannot target a reused node ID")
    @MainActor
    func replacementMakesRetainedHandleStale() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let oldEpoch = inspector.snapshot().epoch
            let retainedHandle = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )

            try await harness.replaceSession(after: oldEpoch)
            let newEpoch = inspector.snapshot().epoch
            #expect(newEpoch != oldEpoch)

            do {
                _ = try await retainedHandle.activate()
                Issue.record("Expected a pre-replacement semantic handle to be stale")
            } catch SemanticAutomationError.staleHandle(let expected, let actual) {
                #expect(expected == oldEpoch)
                #expect(actual == newEpoch)
            } catch {
                Issue.record("Expected staleHandle, got \(error)")
            }

            let freshHandle = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )
            _ = try await freshHandle.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.count == 1)
            #expect(captured.first?.event.nodeId == SemanticInspectionFixture.approveID)
            #expect(captured.first?.event.eventSeq == 1)
        }
    }

    @Test("A queued action rechecks semantic state before outbox admission")
    @MainActor
    func queuedActionRejectsDisablementBeforeOutboxAdmission() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let handle = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )
            let gate = SemanticInspectionActionGate()
            harness.controller.interactionWillEnterOutboxForTesting = {
                await gate.suspend()
            }

            let action = Task { try await handle.activate() }
            await gate.waitUntilEntered()
            try await harness.disableApproveButton()
            await gate.release()

            do {
                _ = try await action.value
                Issue.record("Expected queued activation to observe disablement")
            } catch SemanticAutomationError.nodeDisabled(let nodeID) {
                #expect(nodeID == SemanticInspectionFixture.approveID)
            } catch {
                Issue.record("Expected nodeDisabled, got \(error)")
            }

            harness.controller.interactionWillEnterOutboxForTesting = nil
            let sentinel = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.sentinelLabel)
            )
            _ = try await sentinel.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.map(\.event.nodeId) == [SemanticInspectionFixture.sentinelID])
        }
    }

    @Test("Canceling a queued action prevents outbox admission")
    @MainActor
    func cancelingQueuedActionEmitsNoEvent() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let handle = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )
            let gate = SemanticInspectionActionGate()
            harness.controller.interactionWillEnterOutboxForTesting = {
                await gate.suspend()
            }

            let action = Task { try await handle.activate() }
            await gate.waitUntilEntered()
            action.cancel()
            await gate.release()

            do {
                _ = try await action.value
                Issue.record("Expected canceled activation to throw CancellationError")
            } catch is CancellationError {
                // Expected.
            } catch {
                Issue.record("Expected CancellationError, got \(error)")
            }

            harness.controller.interactionWillEnterOutboxForTesting = nil
            let sentinel = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.sentinelLabel)
            )
            _ = try await sentinel.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.map(\.event.nodeId) == [SemanticInspectionFixture.sentinelID])
        }
    }

    @Test("Canceling before operation registration cannot enqueue an action")
    @MainActor
    func cancelingBeforeOperationRegistrationEmitsNoEvent() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let approve = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )

            let action = Task { try await approve.activate() }
            harness.controller.semanticActionWillRegisterCancellationForTesting = {
                action.cancel()
            }

            do {
                _ = try await action.value
                Issue.record("Expected pre-registration cancellation to throw CancellationError")
            } catch is CancellationError {
                // Expected.
            } catch {
                Issue.record("Expected CancellationError, got \(error)")
            }
            harness.controller.semanticActionWillRegisterCancellationForTesting = nil

            let sentinel = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.sentinelLabel)
            )
            _ = try await sentinel.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.map(\.event.nodeId) == [SemanticInspectionFixture.sentinelID])
            #expect(captured.map(\.event.eventSeq) == [1])
        }
    }

    @Test("Inspected activation flushes pending text before its event")
    @MainActor
    func inspectedActivationFollowsPendingText() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let approve = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )
            try harness.stageDebouncedText("draft")
            #expect(await harness.transport.recordedEvents().isEmpty)

            let gate = SemanticInspectionActionGate()
            harness.controller.interactionWillEnterOutboxForTesting = {
                await gate.suspend()
            }
            let activation = Task { try await approve.activate() }

            await gate.waitUntilEntered()
            #expect(await harness.controller.outbox.eventSeq == 0)
            #expect(
                await harness.transport.recordedEvents().isEmpty,
                "the inspected action must wait behind the flushed text dispatch"
            )

            harness.controller.interactionWillEnterOutboxForTesting = nil
            await gate.release()
            let returned = try await activation.value

            let captured = await harness.transport.recordedEvents()
            #expect(captured.count == 2)
            let text = try #require(captured.first)
            let action = try #require(captured.last)
            #expect(captured.map(\.event.eventType) == [
                .EVENT_TEXT_EDIT,
                .EVENT_ACTIVATE,
            ])
            #expect(captured.map(\.event.eventSeq) == [1, 2])
            #expect(text.logicalClass == .input)
            #expect(text.event.nodeId == SemanticInspectionFixture.editorID)
            #expect(text.event.textArg == "draft")
            #expect(action.logicalClass == .input)
            #expect(action.event.nodeId == SemanticInspectionFixture.approveID)
            #expect(returned == action.event)
        }
    }

    @Test("An inspected action rechecks state after pending text drains")
    @MainActor
    func inspectedActionRechecksAfterPendingTextDrain() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let approve = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )
            try harness.stageDebouncedText("before-disable")

            let gate = SemanticInspectionActionGate()
            harness.controller.semanticActionDidDrainTextForTesting = {
                await gate.suspend()
            }
            let activation = Task { try await approve.activate() }

            await gate.waitUntilEntered()
            harness.controller.semanticActionDidDrainTextForTesting = nil
            let beforeDisable = await harness.transport.recordedEvents()
            #expect(beforeDisable.map(\.event.eventType) == [.EVENT_TEXT_EDIT])
            #expect(beforeDisable.map(\.event.nodeId) == [SemanticInspectionFixture.editorID])
            #expect(beforeDisable.first?.event.textArg == "before-disable")

            do {
                try await harness.disableApproveButton()
            } catch {
                await gate.release()
                throw error
            }
            await gate.release()

            do {
                _ = try await activation.value
                Issue.record("Expected activation to recheck state after text draining")
            } catch SemanticAutomationError.nodeDisabled(let nodeID) {
                #expect(nodeID == SemanticInspectionFixture.approveID)
            } catch {
                Issue.record("Expected nodeDisabled, got \(error)")
            }

            let sentinel = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.sentinelLabel)
            )
            _ = try await sentinel.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.map(\.event.eventType) == [
                .EVENT_TEXT_EDIT,
                .EVENT_ACTIVATE,
            ])
            #expect(captured.map(\.event.nodeId) == [
                SemanticInspectionFixture.editorID,
                SemanticInspectionFixture.sentinelID,
            ])
            #expect(captured.map(\.event.eventSeq) == [1, 2])
        }
    }

    @Test("A node that does not emit ACTIVATE is rejected without an outbound event")
    @MainActor
    func unsupportedActionIsRejectedWithoutAnOutboundEvent() async throws {
        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            let progress = try #require(
                inspector.find(role: .progress, label: SemanticInspectionFixture.progressLabel)
            )

            do {
                _ = try await progress.activate()
                Issue.record("Expected Progress activation to be unsupported")
            } catch SemanticAutomationError.unsupportedAction(let nodeID, let eventType) {
                #expect(nodeID == SemanticInspectionFixture.progressID)
                #expect(eventType == .EVENT_ACTIVATE)
            } catch {
                Issue.record("Expected unsupportedAction, got \(error)")
            }

            let sentinel = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.sentinelLabel)
            )
            _ = try await sentinel.activate()

            let captured = await harness.transport.recordedEvents()
            #expect(captured.count == 1)
            #expect(captured.first?.event.nodeId == SemanticInspectionFixture.sentinelID)
            #expect(
                captured.contains {
                    $0.event.nodeId == SemanticInspectionFixture.progressID
                } == false
            )
        }
    }

    @Test("Native semantic diagnostics distinguish races from contract failures")
    func nativeSemanticErrorTaxonomy() {
        let nodeID = SemanticInspectionFixture.approveID
        let eventType = TypeRef.EVENT_ACTIVATE

        #expect(
            SessionController.shouldReportNativeSemanticActionError(.nodeNotFound(nodeID))
        )
        #expect(
            SessionController.shouldReportNativeSemanticActionError(
                .unsupportedAction(nodeID: nodeID, eventType: eventType)
            )
        )
        #expect(
            SessionController.shouldReportNativeSemanticActionError(
                .capabilityNotNegotiated(eventType)
            )
        )
        #expect(
            SessionController.shouldReportNativeSemanticActionError(
                .staleHandle(expected: 1, actual: 2)
            ) == false
        )
        #expect(
            SessionController.shouldReportNativeSemanticActionError(.nodeDisabled(nodeID))
                == false
        )
        #expect(
            SessionController.shouldReportNativeSemanticActionError(.sessionInactive) == false
        )
    }

    @Test("Queued native contract failures emit diagnostics")
    @MainActor
    func queuedNativeContractFailureIsReported() async throws {
        try await withHarness { harness in
            let recorder = SemanticInspectionErrorRecorder()
            harness.controller.nativeSemanticActionErrorReportedForTesting = { error in
                recorder.record(error)
            }
            let gate = SemanticInspectionActionGate()
            harness.controller.interactionWillEnterOutboxForTesting = {
                await gate.suspend()
            }
            let button = try #require(
                harness.renderer.registry.view(for: SemanticInspectionFixture.approveID)
                    as? NSButton
            )

            button.performClick(nil)
            await gate.waitUntilEntered()
            do {
                try await harness.deleteApproveButton()
            } catch {
                harness.controller.interactionWillEnterOutboxForTesting = nil
                await gate.release()
                throw error
            }
            harness.controller.interactionWillEnterOutboxForTesting = nil
            await gate.release()

            try await AsyncTestSupport.eventually(
                description: "native node-not-found diagnostic"
            ) {
                recorder.errors() == [
                    SemanticAutomationError.nodeNotFound(SemanticInspectionFixture.approveID),
                ]
            }
            harness.controller.nativeSemanticActionErrorReportedForTesting = nil
            #expect(await harness.transport.recordedEvents().isEmpty)
        }
    }

    @Test("Retained inspection values do not keep a stopped session alive")
    @MainActor
    func retainedInspectionValuesReleaseStoppedController() async throws {
        var retainedInspector: SemanticInspector?
        var retainedHandle: SemanticNodeHandle?
        weak var stoppedController: SessionController?

        try await withHarness { harness in
            let inspector = harness.controller.makeSemanticInspector()
            stoppedController = harness.controller
            retainedInspector = inspector
            retainedHandle = try #require(
                inspector.find(role: .button, label: SemanticInspectionFixture.approveLabel)
            )
        }

        #expect(stoppedController == nil)
        let inspector = try #require(retainedInspector)
        let handle = try #require(retainedHandle)
        #expect(
            inspector.snapshot().node(SemanticInspectionFixture.approveID)?.label
                == SemanticInspectionFixture.approveLabel
        )

        do {
            _ = try await handle.activate()
            Issue.record("Expected a retained handle to reject action after session release")
        } catch SemanticAutomationError.sessionInactive {
            // Expected.
        } catch {
            Issue.record("Expected sessionInactive, got \(error)")
        }
    }

    @MainActor
    private func withHarness(
        _ body: @MainActor (SemanticInspectionHarness) async throws -> Void
    ) async throws {
        let harness = try await SemanticInspectionHarness.start()
        do {
            try await body(harness)
        } catch {
            await harness.stop()
            throw error
        }
        await harness.stop()
    }
}

private final class SemanticInspectionErrorRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedErrors: [SemanticAutomationError] = []

    func record(_ error: SemanticAutomationError) {
        lock.lock()
        recordedErrors.append(error)
        lock.unlock()
    }

    func errors() -> [SemanticAutomationError] {
        lock.lock()
        defer { lock.unlock() }
        return recordedErrors
    }
}

private actor SemanticInspectionActionGate {
    private var didEnter = false
    private var entryWaiter: CheckedContinuation<Void, Never>?
    private var releaseWaiter: CheckedContinuation<Void, Never>?

    func suspend() async {
        didEnter = true
        entryWaiter?.resume()
        entryWaiter = nil
        await withCheckedContinuation { continuation in
            releaseWaiter = continuation
        }
    }

    func waitUntilEntered() async {
        guard !didEnter else { return }
        await withCheckedContinuation { continuation in
            entryWaiter = continuation
        }
    }

    func release() {
        releaseWaiter?.resume()
        releaseWaiter = nil
    }
}

private enum SemanticInspectionFixture {
    static let surfaceID = NodeId(1)
    static let approveID = NodeId(2)
    static let sentinelID = NodeId(3)
    static let progressID = NodeId(4)
    static let editorID = NodeId(5)

    static let approveLabel = "Approve"
    static let sentinelLabel = "Sentinel"
    static let progressLabel = "Status"

    static var operations: [SemanticModel.Operation] {
        [
            .createNode(id: surfaceID, nodeType: .surface),
            .createNode(
                id: approveID,
                nodeType: .button,
                parentID: surfaceID,
                properties: [
                    Property(property: .label, value: .string(approveLabel)),
                    Property(property: .enabled, value: .bool(true)),
                ]
            ),
            .createNode(
                id: sentinelID,
                nodeType: .button,
                parentID: surfaceID,
                properties: [
                    Property(property: .label, value: .string(sentinelLabel)),
                ]
            ),
            .createNode(
                id: progressID,
                nodeType: .progress,
                parentID: surfaceID,
                properties: [
                    Property(property: .label, value: .string(progressLabel)),
                ]
            ),
            .createNode(
                id: editorID,
                nodeType: .textInput,
                parentID: surfaceID,
                properties: [
                    Property(property: .value, value: .string("")),
                ]
            ),
        ]
    }
}

@MainActor
private final class SemanticInspectionHarness {
    let transport: SemanticInspectionRecordingTransport
    let serverTransport: PipeTransport
    let applier: TransactionApplier
    let renderer: AppKitRenderer
    let controller: SessionController

    private init(
        transport: SemanticInspectionRecordingTransport,
        serverTransport: PipeTransport,
        applier: TransactionApplier,
        renderer: AppKitRenderer,
        controller: SessionController
    ) {
        self.transport = transport
        self.serverTransport = serverTransport
        self.applier = applier
        self.renderer = renderer
        self.controller = controller
    }

    static func start() async throws -> SemanticInspectionHarness {
        let pair = await PipeTransport.createPair()
        let transport = SemanticInspectionRecordingTransport(inner: pair.client)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        let harness = SemanticInspectionHarness(
            transport: transport,
            serverTransport: pair.server,
            applier: applier,
            renderer: renderer,
            controller: controller
        )

        do {
            try await controller.start()
            try await pair.server.send(
                data: try SRUIFraming.encodeFramed(
                    HandshakeFixtures.welcomeMessage(sessionId: "semantic-inspection")
                )
            )

            var mount = SRUIMessage()
            mount.transaction = Transaction(
                baseRevision: .initial,
                newRevision: Revision(1),
                operations: SemanticInspectionFixture.operations
            ).toWire()
            try await pair.server.send(data: try SRUIFraming.encodeFramed(mount))

            try await AsyncTestSupport.eventually(
                description: "semantic inspection fixture mounted"
            ) {
                applier.lastAppliedRevision == Revision(1)
                    && controller.isEventDispatchEnabled
                    && controller.queuedTransactionCountForTesting == 0
                    && renderer.registry.handle(for: SemanticInspectionFixture.approveID) != nil
                    && renderer.registry.handle(for: SemanticInspectionFixture.sentinelID) != nil
                    && renderer.registry.handle(for: SemanticInspectionFixture.progressID) != nil
                    && renderer.registry.handle(for: SemanticInspectionFixture.editorID) != nil
            }
            return harness
        } catch {
            await harness.stop()
            throw error
        }
    }

    func stageDebouncedText(_ text: String) throws {
        let handle = try #require(
            renderer.registry.handle(for: SemanticInspectionFixture.editorID)
        )
        let field = try #require(handle.view as? NSTextField)
        let adapter = try #require(handle.textAdapter)
        field.stringValue = text
        adapter.notifyTextDidChangeForTests()
        #expect(renderer.textEditingSession.localValue(
            for: SemanticInspectionFixture.editorID
        ) == text)
    }

    func deleteApproveButton() async throws {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .deleteNode(id: SemanticInspectionFixture.approveID),
            ]
        ).toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(message))

        try await AsyncTestSupport.eventually(description: "Approve button deleted") {
            applier.lastAppliedRevision == Revision(2)
                && renderer.registry.handle(for: SemanticInspectionFixture.approveID) == nil
        }
    }

    func disableApproveButton() async throws {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: SemanticInspectionFixture.approveID,
                    property: .enabled,
                    value: .bool(false)
                ),
            ]
        ).toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(message))

        try await AsyncTestSupport.eventually(description: "Approve button disabled") {
            applier.lastAppliedRevision == Revision(2)
                && (renderer.registry.view(for: SemanticInspectionFixture.approveID) as? NSButton)?
                    .isEnabled == false
        }
    }

    func replaceSession(after previousEpoch: SemanticInspectionEpoch) async throws {
        var resync = SRUIServerResyncRequired()
        resync.sessionID = "semantic-inspection-replacement"
        resync.snapshotRevision = 1
        resync.reason = "semantic inspection stale-handle coverage"
        resync.continuity = .replaced
        resync.lastProcessedEventSeq = 0
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await controller.handleIncomingMessage(resyncMessage)

        var snapshot = SRUIMessage()
        snapshot.transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: SemanticInspectionFixture.operations
        ).toWire()
        await controller.handleIncomingMessage(snapshot)

        try await AsyncTestSupport.eventually(
            description: "replacement semantic snapshot mounted"
        ) {
            controller.isEventDispatchEnabled
                && controller.makeSemanticInspector().snapshot().epoch != previousEpoch
                && renderer.registry.handle(for: SemanticInspectionFixture.approveID) != nil
        }
    }

    func stop() async {
        await controller.stop()
        await serverTransport.close()
    }
}

private actor SemanticInspectionRecordingTransport: Transport {
    struct RecordedEvent: Sendable {
        let event: Event
        let logicalClass: LogicalChannelClass
    }

    private let inner: PipeTransport
    private var events: [RecordedEvent] = []

    init(inner: PipeTransport) {
        self.inner = inner
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        let message = try decodeFramedMessage(from: data)
        let event: Event?
        if case .event(let wireEvent) = message.msg {
            event = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        } else {
            event = nil
        }

        try await inner.send(data: data, logicalClass: logicalClass)
        if let event {
            events.append(RecordedEvent(event: event, logicalClass: logicalClass))
        }
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        inner.receiveStream()
    }

    func close() async {
        await inner.close()
    }

    func recordedEvents() -> [RecordedEvent] {
        events
    }
}
