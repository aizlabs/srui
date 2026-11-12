// PX-018-G02: collection selection and Tree disclosure events carry the revision the native
// tree shows, not the applier's published snapshot (§7.6, §7.7, §12.2, §18.2, §22.5, §22.7).
//
// The applier publishes a commit several suspensions before the main-actor turn that renders it.
// A List/Table/Tree `SELECTION_CHANGED` (item and cleared forms) or a Tree `EXPANSION_CHANGED`
// made in that gap acted on the rows the user saw, so its `observed_revision` is the last
// rendered revision. `ACTIVATE` and `VALUE_CHANGED` keep the published revision, and the event's
// sequence, identity, retention and replay are unchanged.

import AppKit
import Collections
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import TransportSSH

@Suite("Rendered-revision stamping of collection events (PX-018-G02)")
struct RenderedRevisionStampTests {
    @Test("Selection and disclosure made between publish and render carry the rendered revision")
    @MainActor
    func eventsInThePublishedToRenderedGapCarryTheRenderedRevision() async throws {
        let session = try await RenderedRevisionSession.start(sessionID: "rendered-gap")
        do {
            try await session.mount(revision: 1)
            #expect(session.controller.nativeRenderedRevisionForTesting == Revision(1))

            let gap = try session.armGap(at: Revision(2)) { fixture in
                fixture.selectChild()
                fixture.clearSelection()
                fixture.setParentExpanded(false)
                fixture.activateButton()
            }
            try await session.relabel(to: "two", base: 1)
            #expect(gap.fired)
            #expect(gap.publishedAtGap == Revision(2), "the applier had published the commit")
            #expect(gap.renderedAtGap == Revision(1), "the native tree still showed revision 1")
            #expect(session.controller.nativeRenderedRevisionForTesting == Revision(2))

            let inGap = try await session.events(count: 4)
            #expect(inGap.map(\.eventType) == [
                .EVENT_SELECTION_CHANGED, .EVENT_SELECTION_CHANGED, .EVENT_EXPANSION_CHANGED, .EVENT_ACTIVATE,
            ])
            #expect(inGap[0].arguments == [.VALUE: .itemID(RenderedRevisionFixture.child)])
            #expect(inGap[1].arguments.isEmpty, "the cleared form")
            #expect(inGap.map(\.observedRevision) == [Revision(1), Revision(1), Revision(1), Revision(2)],
                    "collection events say what was shown; ACTIVATE keeps the published revision")
            for event in inGap {
                #expect(event.observedRevision <= gap.publishedAtGap, "never ahead of the applier")
            }

            // After the render the same interactions carry the new revision.
            session.fixture.selectChild()
            session.fixture.clearSelection()
            session.fixture.setParentExpanded(true)
            session.fixture.activateButton()
            let all = try await session.events(count: 8)
            #expect(all.dropFirst(4).map(\.observedRevision) == Array(repeating: Revision(2), count: 4))
            #expect(all.map(\.eventSeq) == Array(1...8), "event_seq is allocated as before")
        } catch {
            await session.stop()
            throw error
        }
        await session.stop()
    }

    @Test("Selection and disclosure made in the turn right after the render carry the new revision")
    @MainActor
    func eventsRightAfterTheRenderCarryTheNewRevision() async throws {
        // The transaction lane records the render several suspensions later
        // (`lastRenderedRevision`); the main actor may run a click before that.
        let session = try await RenderedRevisionSession.start(sessionID: "rendered-after")
        do {
            try await session.mount(revision: 1)
            let before = try session.armGap(at: Revision(2)) { fixture in
                fixture.clearSelection()
            }
            let after = try session.armGap(at: Revision(2), afterRender: true) { fixture in
                fixture.selectChild()
                fixture.clearSelection()
                fixture.setParentExpanded(false)
            }
            try await session.relabel(to: "two", base: 1)
            #expect(before.fired && after.fired)
            #expect(before.renderedAtGap == Revision(1))
            #expect(after.renderedAtGap == Revision(2))
            let events = try await session.events(count: 4)
            #expect(events.map(\.observedRevision) == [Revision(1), Revision(2), Revision(2), Revision(2)])
        } catch {
            await session.stop()
            throw error
        }
        await session.stop()
    }

    @Test("A report the render itself makes carries the revision it renders")
    @MainActor
    func aRevealingPromotionsExpandReportCarriesItsOwnRevision() async throws {
        // The user collapses the parent at revision 1; revision 2 promotes the child, so the
        // render expands the parent and reports it (`expandCollapsedAncestors`) while it applies
        // revision 2. That report describes revision 2, not the state before it.
        // A click in the same commit's publish-to-render gap still carries revision 1.
        let session = try await RenderedRevisionSession.start(sessionID: "rendered-reveal")
        do {
            try await session.mount(revision: 1)
            session.fixture.setParentExpanded(false)
            _ = try await session.events(count: 1)
            _ = try session.armGap(at: Revision(2)) { fixture in
                fixture.clearSelection()
            }
            try await session.promoteChild(base: 1)
            let events = try await session.events(count: 3)
            #expect(events.map(\.eventType) == [
                .EVENT_EXPANSION_CHANGED, .EVENT_SELECTION_CHANGED, .EVENT_EXPANSION_CHANGED,
            ])
            #expect(events.map { $0.arguments[.VALUE] } == [.bool(false), nil, .bool(true)])
            #expect(events.map(\.observedRevision) == [Revision(1), Revision(1), Revision(2)])
        } catch {
            await session.stop()
            throw error
        }
        await session.stop()
    }

    @Test("The catch-up snapshot advances the rendered revision only once it has rendered")
    @MainActor
    func catchUpSnapshotAdvancesTheRenderedRevisionOnlyOnceRendered() async throws {
        let session = try await RenderedRevisionSession.start(
            sessionID: "rendered-catch-up",
            initialRevision: 3
        )
        do {
            let beforeMount = RenderedRevisionSample()
            let controller = session.controller
            let applier = session.applier
            controller.rendererResourcesPreloadedInterceptorForTesting = {
                await beforeMount.record(controller: controller, applier: applier)
            }
            try await session.mount(revision: 3)
            controller.rendererResourcesPreloadedInterceptorForTesting = nil
            #expect(beforeMount.samples.first?.published == Revision(3), "the snapshot was published")
            #expect(beforeMount.samples.first?.rendered == Revision.initial, "and not yet rendered")
            #expect(controller.nativeRenderedRevisionForTesting == Revision(3))

            let gap = try session.armGap(at: Revision(4)) { fixture in
                fixture.selectChild()
                fixture.setParentExpanded(false)
            }
            try await session.relabel(to: "four", base: 3)
            #expect(gap.renderedAtGap == Revision(3))
            session.fixture.clearSelection()
            let events = try await session.events(count: 3)
            #expect(events.map(\.observedRevision) == [Revision(3), Revision(3), Revision(4)])
        } catch {
            await session.stop()
            throw error
        }
        await session.stop()
    }

    @Test("A same-session resume remounts before it advances, and a retained event keeps its revision")
    @MainActor
    func resumeAdvancesOnlyOnceRenderedAndReplayKeepsTheOriginalRevision() async throws {
        let sessionID = "rendered-resume"
        let first = try await RenderedRevisionSession.start(sessionID: sessionID)
        let retained: [Event]
        do {
            try await first.mount(revision: 1)
            _ = try first.armGap(at: Revision(2)) { fixture in
                fixture.selectChild()
                fixture.setParentExpanded(false)
            }
            try await first.relabel(to: "two", base: 1)
            // The server never acknowledges them, so the outbox retains both.
            retained = try await first.events(count: 2)
            #expect(retained.map(\.observedRevision) == [Revision(1), Revision(1)])
        } catch {
            await first.stop()
            throw error
        }
        await first.stop()

        let second = try await RenderedRevisionSession.resume(after: first, sessionID: sessionID)
        do {
            try await AsyncTestSupport.eventuallyAsync(
                timeout: .roundTrip,
                description: "resume remounted and replayed the retained events"
            ) {
                await second.transport.recordedEvents().count >= 2
                    && second.controller.isEventDispatchEnabled
                    && second.controller.nativeRenderedRevisionForTesting == Revision(2)
            }
            #expect(second.resumeSample.samples.first?.published == Revision(2))
            #expect(second.resumeSample.samples.first?.rendered == Revision.initial,
                    "the resumed controller had rendered nothing before its remount")
            let replayed = Array(try await second.events(count: 2).prefix(2))
            #expect(replayed.map(\.eventId) == retained.map(\.eventId))
            #expect(replayed.map(\.eventSeq) == retained.map(\.eventSeq))
            #expect(replayed.map(\.observedRevision) == [Revision(1), Revision(1)],
                    "a replay resends the revision the event was made at")

            let gap = try second.armGap(at: Revision(3)) { fixture in
                fixture.clearSelection()
            }
            try await second.relabel(to: "three", base: 2)
            #expect(gap.renderedAtGap == Revision(2))
            second.fixture.selectChild()
            // This server never acknowledges, so the resumed outbox's replay loop resends every
            // unsettled event from 1 s on (`PendingEventReplayLoop`), and a slow run sees those
            // resends here: [2, 1, 1, 2, 3] under CPU load. Count each event once, at its first
            // send, and require every resend to keep the revision it was made at.
            let all = try await second.firstSends(count: 4)
            #expect(all.dropFirst(2).map(\.observedRevision) == [Revision(2), Revision(3)])
            let firstRevision = Dictionary(uniqueKeysWithValues: all.map { ($0.eventId, $0.observedRevision) })
            for sent in await second.transport.recordedEvents() {
                #expect(firstRevision[sent.eventId] == sent.observedRevision, "a resend keeps its revision")
            }
        } catch {
            await second.stop()
            throw error
        }
        await second.stop()
    }

    @Test("Only collection selection and Tree disclosure use the rendered revision, never ahead of the applier")
    func stampingScopeAndClamp() {
        for nodeType in [TypeRef.list, .table, .tree] {
            #expect(SessionController.stampsRenderedRevision(.selectionChanged(ItemId(1)), on: nodeType))
            #expect(SessionController.stampsRenderedRevision(.selectionCleared, on: nodeType))
        }
        #expect(SessionController.stampsRenderedRevision(.expansionChanged(ItemId(1), expanded: false), on: .tree))
        for nodeType in [TypeRef.select, .choiceGroup, .tabs] {
            #expect(!SessionController.stampsRenderedRevision(.selectionChanged(ItemId(1)), on: nodeType))
        }
        #expect(!SessionController.stampsRenderedRevision(.activate, on: .button))
        #expect(!SessionController.stampsRenderedRevision(.valueChanged(.bool(true)), on: .toggle))
        #expect(!SessionController.stampsRenderedRevision(.selectionChanged(ItemId(1)), on: nil))

        #expect(SessionController.renderedObservedRevision(rendered: Revision(4), published: Revision(5)) == Revision(4))
        #expect(SessionController.renderedObservedRevision(rendered: Revision(5), published: Revision(5)) == Revision(5))
        // A replica reset publishes `.initial` before the main actor resets what it shows.
        #expect(SessionController.renderedObservedRevision(rendered: Revision(9), published: .initial) == .initial)
    }
}

private enum RenderedRevisionFixture {
    static let surfaceID = NodeId(1)
    static let buttonID = NodeId(2)
    static let treeID = NodeId(7)
    static let treeModelID = ModelId(70)
    static let parent = ItemId(701)
    static let child = ItemId(702)

    static var operations: [SemanticModel.Operation] {
        [
            .createNode(id: surfaceID, nodeType: .surface),
            .createNode(
                id: buttonID,
                nodeType: .button,
                parentID: surfaceID,
                properties: [Property(property: .label, value: .string("one"))]
            ),
            .createModel(id: treeModelID, modelType: .tree, itemCount: 0),
            .modelInsert(
                id: treeModelID,
                index: 0,
                items: [
                    ModelItem(
                        itemID: parent,
                        value: .string("src"),
                        properties: [.treeDepth: .unsignedInt(0), .descendantCount: .unsignedInt(1)]
                    ),
                    ModelItem(
                        itemID: child,
                        value: .string("main.swift"),
                        properties: [.treeDepth: .unsignedInt(1), .descendantCount: .unsignedInt(0)]
                    ),
                ]
            ),
            .createNode(
                id: treeID,
                nodeType: .tree,
                parentID: surfaceID,
                properties: [
                    Property(property: .label, value: .string("Files")),
                    Property(property: .modelRef, value: .unsignedInt(treeModelID.value)),
                    Property(property: .selectionMode, value: .enumToken(.selectionModeSingle)),
                ]
            ),
        ]
    }
}

/// The native interactions a test makes, through the renderer's own callbacks.
@MainActor
private struct RenderedRevisionInteractions {
    let renderer: AppKitRenderer

    private var tree: (OutlineCollectionAdapter, NSOutlineView)? {
        guard let handle = renderer.registry.handle(for: RenderedRevisionFixture.treeID),
              let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
              let outline = (handle.view as? NSScrollView)?.documentView as? NSOutlineView else {
            return nil
        }
        return (adapter, outline)
    }

    func selectChild() {
        renderer.onInteraction?(.selectionChanged(
            nodeID: RenderedRevisionFixture.treeID,
            itemID: RenderedRevisionFixture.child
        ))
    }

    func clearSelection() {
        renderer.onInteraction?(.selectionCleared(nodeID: RenderedRevisionFixture.treeID))
    }

    /// The disclosure triangle's own path (`OutlineCollectionAdapter.setExpanded`).
    func setParentExpanded(_ expanded: Bool) {
        guard let (adapter, outline) = tree else {
            Issue.record("the tree is not rendered")
            return
        }
        #expect(adapter.setExpanded(expanded, itemID: RenderedRevisionFixture.parent, outlineView: outline))
    }

    func activateButton() {
        guard let button = renderer.registry.view(for: RenderedRevisionFixture.buttonID) as? NSButton else {
            Issue.record("the button is not rendered")
            return
        }
        NativeActivation.click(button)
    }
}

/// Runs interactions once, on the main actor, after the applier has published `revision` and
/// before the renderer applies it (`liveTransactionPublishedInterceptorForTesting`).
@MainActor
private final class RenderedRevisionGap {
    let revision: Revision
    let applier: TransactionApplier
    let controller: SessionController
    let interactions: RenderedRevisionInteractions
    let body: @MainActor (RenderedRevisionInteractions) -> Void
    private(set) var fired = false
    private(set) var publishedAtGap = Revision.initial
    private(set) var renderedAtGap = Revision.initial

    init(
        revision: Revision,
        applier: TransactionApplier,
        controller: SessionController,
        interactions: RenderedRevisionInteractions,
        body: @escaping @MainActor (RenderedRevisionInteractions) -> Void
    ) {
        self.revision = revision
        self.applier = applier
        self.controller = controller
        self.interactions = interactions
        self.body = body
    }

    func fire() {
        guard !fired, applier.lastAppliedRevision == revision else { return }
        fired = true
        publishedAtGap = applier.lastAppliedRevision
        renderedAtGap = controller.nativeRenderedRevisionForTesting
        body(interactions)
    }
}

/// What the native tree showed, and what the applier had published, at an interceptor.
@MainActor
private final class RenderedRevisionSample {
    private(set) var samples: [(rendered: Revision, published: Revision)] = []

    func record(controller: SessionController, applier: TransactionApplier) {
        samples.append((controller.nativeRenderedRevisionForTesting, applier.lastAppliedRevision))
    }
}

@MainActor
private final class RenderedRevisionSession {
    let transport: RenderedRevisionRecordingTransport
    let server: PipeTransport
    let applier: TransactionApplier
    let outbox: EventOutbox
    let renderer: AppKitRenderer
    let controller: SessionController
    let resumeSample = RenderedRevisionSample()
    /// Gaps armed for the next commit; it counts as delivered only once each has fired, because
    /// the after-render interceptor runs a hop after the render the waits below observe.
    private var armed: [RenderedRevisionGap] = []

    var fixture: RenderedRevisionInteractions { RenderedRevisionInteractions(renderer: renderer) }

    private init(
        transport: RenderedRevisionRecordingTransport,
        server: PipeTransport,
        applier: TransactionApplier,
        outbox: EventOutbox,
        renderer: AppKitRenderer,
        sessionID: String?
    ) {
        self.transport = transport
        self.server = server
        self.applier = applier
        self.outbox = outbox
        self.renderer = renderer
        controller = SessionController(
            transport: transport,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        controller.attachRenderer(renderer)
    }

    /// A fresh session: WELCOME, then (from `mount`) its first transaction or catch-up snapshot.
    static func start(sessionID: String, initialRevision: UInt64 = 0) async throws -> RenderedRevisionSession {
        let pair = await PipeTransport.createPair()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let session = RenderedRevisionSession(
            transport: RenderedRevisionRecordingTransport(inner: pair.client),
            server: pair.server,
            applier: TransactionApplier(),
            outbox: EventOutbox(),
            renderer: renderer,
            sessionID: nil
        )
        try await session.controller.start()
        var welcome = HandshakeFixtures.welcomeMessage(sessionId: sessionID)
        welcome.serverWelcome.initialRevision = initialRevision
        try await session.server.send(data: try SRUIFraming.encodeFramed(welcome))
        return session
    }

    /// A same-session resume of `previous`'s replica, outbox and renderer on a new transport.
    static func resume(after previous: RenderedRevisionSession, sessionID: String) async throws -> RenderedRevisionSession {
        let pair = await PipeTransport.createPair()
        let session = RenderedRevisionSession(
            transport: RenderedRevisionRecordingTransport(inner: pair.client),
            server: pair.server,
            applier: previous.applier,
            outbox: previous.outbox,
            renderer: previous.renderer,
            sessionID: sessionID
        )
        let controller = session.controller
        let applier = session.applier
        let sample = session.resumeSample
        controller.resumeRecoverySnapshotLoadedInterceptorForTesting = {
            await sample.record(controller: controller, applier: applier)
        }
        try await controller.start()
        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = sessionID
        resumeOk.lastProcessedEventSeq = 0
        var message = SRUIMessage()
        message.serverResumeOk = resumeOk
        try await session.server.send(data: try SRUIFraming.encodeFramed(message))
        return session
    }

    func mount(revision: UInt64) async throws {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(revision),
            operations: RenderedRevisionFixture.operations
        ).toWire()
        try await server.send(data: try SRUIFraming.encodeFramed(message))
        let renderer = renderer
        let applier = applier
        let controller = controller
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "fixture mounted") {
            applier.lastAppliedRevision == Revision(revision)
                && controller.isEventDispatchEnabled
                && renderer.registry.handle(for: RenderedRevisionFixture.treeID) != nil
                && (renderer.registry.view(for: RenderedRevisionFixture.buttonID) as? NSButton) != nil
        }
    }

    /// Arms an interceptor for the next live commit, which `relabel` delivers: after the applier
    /// publishes it and before the render, or with `afterRender` right after the main-actor render
    /// and before the transaction lane resumes (`rendererDidRenderInterceptorForTesting`).
    func armGap(
        at revision: Revision,
        afterRender: Bool = false,
        _ body: @escaping @MainActor (RenderedRevisionInteractions) -> Void
    ) throws -> RenderedRevisionGap {
        let gap = RenderedRevisionGap(
            revision: revision,
            applier: applier,
            controller: controller,
            interactions: fixture,
            body: body
        )
        if afterRender {
            controller.rendererDidRenderInterceptorForTesting = { await gap.fire() }
        } else {
            controller.liveTransactionPublishedInterceptorForTesting = { await gap.fire() }
        }
        armed.append(gap)
        return gap
    }

    private func disarm() async throws {
        let gaps = armed
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "every armed gap fired") {
            gaps.allSatisfy(\.fired)
        }
        armed = []
        controller.liveTransactionPublishedInterceptorForTesting = nil
        controller.rendererDidRenderInterceptorForTesting = nil
    }

    /// Delivers `base + 1`, which retitles the button, and waits until it has rendered.
    func relabel(to label: String, base: UInt64) async throws {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: Revision(base),
            newRevision: Revision(base + 1),
            operations: [
                .setProperty(id: RenderedRevisionFixture.buttonID, property: .label, value: .string(label)),
            ]
        ).toWire()
        try await server.send(data: try SRUIFraming.encodeFramed(message))
        let renderer = renderer
        let controller = controller
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "revision \(base + 1) rendered") {
            (renderer.registry.view(for: RenderedRevisionFixture.buttonID) as? NSButton)?.title == label
                && controller.nativeRenderedRevisionForTesting == Revision(base + 1)
        }
        try await disarm()
    }

    /// Delivers `base + 1`, which promotes the child as the tree's selection, and waits until the
    /// outline has revealed and selected it.
    func promoteChild(base: UInt64) async throws {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: Revision(base),
            newRevision: Revision(base + 1),
            operations: [
                .setProperty(
                    id: RenderedRevisionFixture.treeID,
                    property: .value,
                    value: .itemID(RenderedRevisionFixture.child)
                ),
            ]
        ).toWire()
        try await server.send(data: try SRUIFraming.encodeFramed(message))
        let renderer = renderer
        let controller = controller
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "revision \(base + 1) revealed the child") {
            guard let handle = renderer.registry.handle(for: RenderedRevisionFixture.treeID),
                  let adapter = handle.modelAdapter as? OutlineCollectionAdapter else { return false }
            return adapter.isExpanded(RenderedRevisionFixture.parent)
                && controller.nativeRenderedRevisionForTesting == Revision(base + 1)
        }
        try await disarm()
    }

    /// Waits for `count` events on this transport and returns them in send order.
    func events(count: Int) async throws -> [Event] {
        let transport = transport
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "\(count) events sent") {
            await transport.recordedEvents().count >= count
        }
        return await transport.recordedEvents()
    }

    /// Waits for `count` distinct events and returns each at its first send, in send order: a
    /// replay-loop resend of an unacknowledged event repeats an event already counted.
    func firstSends(count: Int) async throws -> [Event] {
        let transport = transport
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "\(count) distinct events sent") {
            Self.firstSends(of: await transport.recordedEvents()).count >= count
        }
        return Self.firstSends(of: await transport.recordedEvents())
    }

    private static func firstSends(of events: [Event]) -> [Event] {
        var seen = Set<EventId>()
        return events.filter { seen.insert($0.eventId).inserted }
    }

    func stop() async {
        await controller.stop()
        await server.close()
    }
}

private actor RenderedRevisionRecordingTransport: Transport {
    private let inner: PipeTransport
    private var events: [Event] = []

    init(inner: PipeTransport) {
        self.inner = inner
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        let message = try decodeFramedMessage(from: data)
        try await inner.send(data: data, logicalClass: logicalClass)
        if case .event(let wireEvent) = message.msg {
            events.append(try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent))
        }
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        inner.receiveStream()
    }

    func close() async {
        await inner.close()
    }

    func recordedEvents() -> [Event] {
        events
    }
}
