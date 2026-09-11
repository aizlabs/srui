//
// CollectionRangeSessionTests.swift
// SRUITests
//
// Non-blocking cache-miss requests: AppKit stays on the main actor while `.ui` send is gated (§8, §22.2, §22.7).
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit
import Collections

@Suite("Collection range request pump")
struct CollectionRangeSessionTests {

    @Test("Scroll-generated range request returns before a gated UI send completes")
    @MainActor
    func gatedUISendDoesNotBlockVisibleRangeCallback() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let gated = UIGatingTransport(inner: clientPipe)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: gated,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "range-pump-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        let surfaceID = NodeId(1)
        let tableID = NodeId(12)
        let modelID = ModelId(99)
        let mountTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createModel(id: modelID, modelType: .table, itemCount: 500_000),
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: tableID,
                    nodeType: .table,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .modelRef, value: .unsignedInt(modelID.value)),
                        Property(property: .columns, value: .list([.string("Label")])),
                    ]
                ),
            ]
        )
        var mountMsg = SRUIMessage()
        mountMsg.transaction = mountTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(mountMsg))

        try await AsyncTestSupport.eventually(description: "table mounted") {
            applier.lastAppliedRevision == Revision(1)
                && renderer.registry.handle(for: tableID) != nil
        }

        let handle = try #require(renderer.registry.handle(for: tableID))
        let scroll = try #require(handle.view as? NSScrollView)
        let tableView = try #require(scroll.documentView as? NSTableView)
        let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)
        let tableBefore = tableView
        #expect(type(of: tableView) == NSTableView.self)
        #expect(adapter.numberOfRows(in: tableView) == 500_000)

        let loading = try #require(
            adapter.tableView(tableView, viewFor: tableView.tableColumns[0], row: 0) as? NSTextField
        )
        #expect(loading.stringValue == CollectionCells.loadingPlaceholder)

        var sentinelRan = false
        Task { @MainActor in sentinelRan = true }
        adapter.noteVisibleRange(start: 0, count: 8)

        try await AsyncTestSupport.eventually(description: "MainActor sentinel") { sentinelRan }
        let uiDeadline = Date().addingTimeInterval(2)
        while Date() < uiDeadline {
            if await gated.uiFrameCount == 1 {
                break
            }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        #expect(await gated.uiFrameCount == 1)
        #expect(await gated.isUISendBlocked)
        let framed = try #require(await gated.uiFrames.first)
        let decoded = try SRUIFraming.decodeFramed(SRUIMessage.self, from: framed)
        guard case .clientModelRangeRequest(let request)? = decoded.msg else {
            Issue.record("expected ClientModelRangeRequest, got \(String(describing: decoded.msg))")
            return
        }
        #expect(request.nodeID == tableID.value)
        #expect(request.modelID == modelID.value)
        #expect(request.startIndex == 0)
        #expect(request.count == 128)
        #expect(request.observedRevision == 1)
        #expect(adapter.rowContent(at: 0)?.itemID == nil)

        await gated.releaseUI()

        let items = (0..<128).map { index in
            ModelItem(itemID: ItemId(index + 1), value: .string("Row \(index)"))
        }
        let resetTx = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .modelResetRange(id: modelID, startIndex: 0, items: items, totalCount: 500_000)
            ]
        )
        var resetMsg = SRUIMessage()
        resetMsg.transaction = resetTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resetMsg))

        try await AsyncTestSupport.eventually(description: "range arrival painted") {
            adapter.rowContent(at: 0)?.cells == ["Row 0"]
                && adapter.rowContent(at: 0)?.itemID == ItemId(1)
        }
        let tableAfter = try #require((renderer.registry.view(for: tableID) as? NSScrollView)?.documentView as? NSTableView)
        #expect(tableAfter === tableBefore)
        #expect(adapter.numberOfRows(in: tableAfter) == 500_000)

        await controller.stop()
        await serverTransport.close()
    }
}

actor UIGatingTransport: Transport {
    let inner: PipeTransport
    private let stream: AsyncThrowingStream<Data, Error>
    private var uiWaiters: [CheckedContinuation<Void, Never>] = []
    private(set) var uiFrames: [Data] = []

    init(inner: PipeTransport) {
        self.inner = inner
        self.stream = inner.receiveStream()
    }

    var uiFrameCount: Int { uiFrames.count }
    var isUISendBlocked: Bool { !uiWaiters.isEmpty }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        if logicalClass == .ui {
            uiFrames.append(data)
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                uiWaiters.append(continuation)
            }
        }
        try await inner.send(data: data, logicalClass: logicalClass)
    }

    func releaseUI() {
        let waiters = uiWaiters
        uiWaiters.removeAll()
        for waiter in waiters {
            waiter.resume()
        }
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        releaseUI()
        await inner.close()
    }
}
