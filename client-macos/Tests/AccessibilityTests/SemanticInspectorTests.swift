import Foundation
import Testing
import Accessibility
import SemanticModel

@Suite("Semantic inspection")
struct SemanticInspectorTests {
    @Test("Snapshot preserves deterministic hierarchy and semantic fields")
    func snapshotHierarchyAndFields() throws {
        let source = try makeSource(revision: 7, epoch: 3)
        let inspector = makeInspector(source: source)

        let snapshot = inspector.snapshot()

        #expect(snapshot.revision == Revision(7))
        #expect(snapshot.epoch == SemanticInspectionEpoch(3))
        #expect(snapshot.rootIDs == [NodeId(1)])
        #expect(snapshot.nodes.map { $0.id } == [NodeId(1), NodeId(2), NodeId(3), NodeId(4), NodeId(5)])

        let root = try #require(snapshot.node(NodeId(1)))
        #expect(root.index == 0)
        #expect(root.depth == 1)
        #expect(root.childIDs == [NodeId(2)])

        let button = try #require(snapshot.node(NodeId(4)))
        #expect(button.role == TypeRef.button)
        #expect(button.roleHint == EnumToken.actionRolePrimary)
        #expect(button.label == "Approve")
        #expect(button.text == "Approve request")
        #expect(button.accessibleDescription == "Accept the pending request")
        #expect(button.valueDescription == "Not yet approved")
        #expect(button.value == Value.bool(false))
        #expect(button.parentID == NodeId(2))
        #expect(button.index == 1)
        #expect(button.depth == 3)
        #expect(button.state.visibility == .visibilityVisible)
        #expect(button.state.enabled)
        #expect(button.state.readOnly)
        #expect(button.state.busy)
        #expect(button.state.selected == false)
        #expect(button.actionKey == "approve")
        #expect(button.actions == ["announce", "show_details"])
        #expect(snapshot[NodeId(4)] == button)
        #expect(snapshot[NodeId(404)] == nil)
    }

    @Test("Each query takes a fresh source while old snapshots remain immutable")
    func freshAndImmutableSnapshots() throws {
        let firstSource = try makeSource(buttonLabel: "Before", revision: 1, epoch: 9)
        let secondSource = try makeSource(buttonLabel: "After", revision: 2, epoch: 10)
        let sequence = LockedSnapshotSequence([firstSource, secondSource])
        let inspector = SemanticInspector(
            snapshotProvider: { sequence.next() },
            actionHandler: { _ in throw SemanticAutomationError.sessionInactive }
        )

        let first = inspector.snapshot()
        let second = inspector.snapshot()

        #expect(first.node(NodeId(4))?.label == "Before")
        #expect(first.revision == Revision(1))
        #expect(first.epoch == SemanticInspectionEpoch(9))
        #expect(second.node(NodeId(4))?.label == "After")
        #expect(second.revision == Revision(2))
        #expect(second.epoch == SemanticInspectionEpoch(10))
        #expect(first.node(NodeId(4))?.label == "Before")
    }

    @Test("Existing snapshots mint handles only through their originating inspector")
    func existingSnapshotHandlesRequireOrigin() throws {
        let source = try makeSource(buttonLabel: "Before", revision: 11, epoch: 20)
        let sequence = LockedSnapshotSequence([source])
        let inspector = SemanticInspector(
            snapshotProvider: { sequence.next() },
            actionHandler: { _ in throw SemanticAutomationError.sessionInactive }
        )
        let otherInspector = makeInspector(source: source)

        let tree = inspector.snapshot()
        let handle = try #require(inspector.handle(for: NodeId(4), in: tree))

        #expect(sequence.consumedCount() == 1)
        #expect(handle.node == tree.node(NodeId(4)))
        #expect(handle.observedRevision == Revision(11))
        #expect(handle.expectedEpoch == SemanticInspectionEpoch(20))
        #expect(otherInspector.handle(for: NodeId(4), in: tree) == nil)

        let fabricated = SemanticTreeSnapshot(
            epoch: tree.epoch,
            revision: tree.revision,
            rootIDs: tree.rootIDs,
            nodes: tree.nodes
        )
        #expect(fabricated == tree)
        #expect(inspector.handle(for: NodeId(4), in: fabricated) == nil)
        #expect(sequence.consumedCount() == 1)
    }

    @Test("Find uses role and label in semantic tree order")
    func findByRoleAndLabel() throws {
        let source = try makeSource(revision: 4, epoch: 12)
        let inspector = makeInspector(source: source)

        let approve = try #require(inspector.find(role: TypeRef.button, label: "Approve"))
        #expect(approve.id == NodeId(4))
        #expect(approve.role == TypeRef.button)
        #expect(approve.label == "Approve")
        #expect(inspector.find(role: TypeRef.button, label: "Missing") == nil)
        #expect(inspector.findAll(role: TypeRef.text).map { $0.id } == [NodeId(3)])
    }

    @Test("Snapshot event capabilities come from the registry, not passive actions")
    func registryActions() throws {
        #expect(standardEventsEmitted(by: TypeRef(namespaceID: 42, localID: 11)).isEmpty)

        let snapshot = makeInspector(source: try makeSource(revision: 1, epoch: 1)).snapshot()
        let button = try #require(snapshot.node(NodeId(4)))
        let menu = try #require(snapshot.node(NodeId(5)))
        #expect(button.supportedEventTypes == standardEventsEmitted(by: button.role))
        #expect(menu.supportedEventTypes == standardEventsEmitted(by: menu.role))
        #expect(menu.actionKey == "menu.command")
        #expect(menu.actions == ["activate"])
    }

    @Test("Handles forward typed actions with their captured node, epoch, and revision")
    func handleForwardsRequests() async throws {
        let firstSource = try makeSource(revision: 11, epoch: 27)
        let secondSource = try makeSource(
            buttonLabel: "Approve updated",
            revision: 12,
            epoch: 27
        )
        let sequence = LockedSnapshotSequence([firstSource, secondSource])
        let recorder = ActionRequestRecorder()
        let inspector = SemanticInspector(
            snapshotProvider: { sequence.next() },
            actionHandler: { request in
                await recorder.receive(request)
            }
        )
        let handle = try #require(inspector.find(role: TypeRef.button, label: "Approve"))
        let freshHandle = try #require(
            inspector.find(role: TypeRef.button, label: "Approve updated")
        )
        #expect(handle.observedRevision == Revision(11))
        #expect(freshHandle.observedRevision == Revision(12))

        let activateEvent = try await handle.activate()
        let valueEvent = try await handle.setValue(.bool(true))
        let selectionEvent = try await handle.select(ItemId(88))
        let freshEvent = try await freshHandle.activate()
        let requests = await recorder.requests()

        #expect(requests == [
            SemanticActionRequest(
                nodeID: NodeId(4),
                expectedEpoch: SemanticInspectionEpoch(27),
                observedRevision: Revision(11),
                action: .activate
            ),
            SemanticActionRequest(
                nodeID: NodeId(4),
                expectedEpoch: SemanticInspectionEpoch(27),
                observedRevision: Revision(11),
                action: .valueChanged(.bool(true))
            ),
            SemanticActionRequest(
                nodeID: NodeId(4),
                expectedEpoch: SemanticInspectionEpoch(27),
                observedRevision: Revision(11),
                action: .selectionChanged(ItemId(88))
            ),
            SemanticActionRequest(
                nodeID: NodeId(4),
                expectedEpoch: SemanticInspectionEpoch(27),
                observedRevision: Revision(12),
                action: .activate
            ),
        ])
        #expect(activateEvent.eventType == TypeRef.EVENT_ACTIVATE)
        #expect(activateEvent.observedRevision == Revision(11))
        #expect(valueEvent.eventType == TypeRef.EVENT_VALUE_CHANGED)
        #expect(valueEvent.arguments[PropertyRef.VALUE] == Value.bool(true))
        #expect(selectionEvent.eventType == TypeRef.EVENT_SELECTION_CHANGED)
        #expect(selectionEvent.arguments[PropertyRef.VALUE] == Value.itemID(ItemId(88)))
        #expect(freshEvent.observedRevision == Revision(12))
    }
}

private func makeInspector(source: SemanticInspectionSourceSnapshot) -> SemanticInspector {
    SemanticInspector(
        snapshotProvider: { source },
        actionHandler: { _ in throw SemanticAutomationError.sessionInactive }
    )
}

private func makeSource(
    buttonLabel: String = "Approve",
    revision: Revision,
    epoch: SemanticInspectionEpoch
) throws -> SemanticInspectionSourceSnapshot {
    var store = SemanticStore()
    try store.createNode(
        id: NodeId(1),
        nodeType: .surface,
        properties: [(.label, .string("Window"))]
    )
    try store.createNode(
        id: NodeId(2),
        nodeType: .column,
        parentID: NodeId(1)
    )
    try store.createNode(
        id: NodeId(3),
        nodeType: .text,
        parentID: NodeId(2),
        properties: [(.text, .string("Pending request"))]
    )
    try store.createNode(
        id: NodeId(4),
        nodeType: .button,
        parentID: NodeId(2),
        properties: [
            (.label, .string(buttonLabel)),
            (.role, .enumToken(.actionRolePrimary)),
            (.text, .string("Approve request")),
            (.accessibleDescription, .string("Accept the pending request")),
            (.valueDescription, .string("Not yet approved")),
            (.value, .bool(false)),
            (.enabled, .bool(true)),
            (.readOnly, .bool(true)),
            (.busy, .bool(true)),
            (.selected, .bool(false)),
            (.actionKey, .string("approve")),
            (.actions, .list([.string("announce"), .string("show_details")])),
        ]
    )
    try store.createNode(
        id: NodeId(5),
        nodeType: .menu,
        parentID: NodeId(2),
        properties: [
            (.label, .string("Commands")),
            (.actionKey, .string("menu.command")),
            (.actions, .list([.string("activate")])),
        ]
    )

    return SemanticInspectionSourceSnapshot(
        transaction: TransactionSnapshot(store: store, revision: revision),
        epoch: epoch
    )
}

private final class LockedSnapshotSequence: @unchecked Sendable {
    private let lock = NSLock()
    private let snapshots: [SemanticInspectionSourceSnapshot]
    private var index = 0

    init(_ snapshots: [SemanticInspectionSourceSnapshot]) {
        precondition(!snapshots.isEmpty)
        self.snapshots = snapshots
    }

    func next() -> SemanticInspectionSourceSnapshot {
        lock.lock()
        defer { lock.unlock() }
        let snapshot = snapshots[min(index, snapshots.count - 1)]
        index += 1
        return snapshot
    }

    func consumedCount() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return index
    }
}

private actor ActionRequestRecorder {
    private var recorded: [SemanticActionRequest] = []

    func receive(_ request: SemanticActionRequest) -> Event {
        recorded.append(request)

        let arguments: [PropertyRef: Value]
        switch request.action {
        case .activate:
            arguments = [:]
        case .valueChanged(let value):
            arguments = [.VALUE: value]
        case .selectionChanged(let itemID):
            arguments = [.VALUE: .itemID(itemID)]
        }

        return Event(
            eventSeq: UInt64(recorded.count),
            eventId: EventId(string: "semantic-test-\(recorded.count)"),
            observedRevision: request.observedRevision,
            nodeId: request.nodeID,
            eventType: request.action.eventType,
            arguments: arguments
        )
    }

    func requests() -> [SemanticActionRequest] {
        recorded
    }
}
