import SemanticModel
import Testing
@testable import Collections

struct CollectionRangeTrackerTests {
    @Test
    func alignedWindowUsesPageSizeAndPrefetchAndClipsAtItemCount() {
        #expect(
            CollectionRangeTracker.alignedWindow(
                visibleStart: 10,
                visibleCount: 5,
                itemCount: 500_000
            ) == 0..<128
        )
        #expect(
            CollectionRangeTracker.alignedWindow(
                visibleStart: 200,
                visibleCount: 10,
                itemCount: 500_000
            ) == 128..<256
        )
        #expect(
            CollectionRangeTracker.alignedWindow(
                visibleStart: 499_990,
                visibleCount: 20,
                itemCount: 500_000
            ) == 499_840..<500_000
        )
        #expect(
            CollectionRangeTracker.alignedWindow(
                visibleStart: 0,
                visibleCount: 0,
                itemCount: 500_000
            ) == nil
        )
    }

    @Test
    func repeatedRequestsForTheSamePageEmitOnceUntilResetOrArrival() throws {
        var store = SemanticStore()
        let modelID = ModelId(1)
        let nodeID = NodeId(2)
        try store.createModel(id: modelID, modelType: .table, itemCount: 500_000)
        let model = try #require(store.getModel(modelID))

        var tracker = CollectionRangeTracker()
        let first = tracker.requests(
            visibleStart: 10,
            visibleCount: 5,
            itemCount: 500_000,
            model: model,
            nodeID: nodeID,
            modelID: modelID
        )
        #expect(first == [
            CollectionRangeRequest(nodeID: nodeID, modelID: modelID, startIndex: 0, count: 128)
        ])
        let second = tracker.requests(
            visibleStart: 12,
            visibleCount: 5,
            itemCount: 500_000,
            model: model,
            nodeID: nodeID,
            modelID: modelID
        )
        #expect(second.isEmpty)

        tracker.noteArrived(start: 0, count: 128)
        try store.modelResetRange(
            id: modelID,
            startIndex: 0,
            items: (0..<128).map { ModelItem(itemID: ItemId($0 + 1), value: .string("r\($0)")) },
            totalCount: 500_000
        )
        let hydrated = try #require(store.getModel(modelID))
        let third = tracker.requests(
            visibleStart: 10,
            visibleCount: 5,
            itemCount: 500_000,
            model: hydrated,
            nodeID: nodeID,
            modelID: modelID
        )
        #expect(third.isEmpty)
    }

    @Test
    func pendingSegmentsLeaveThePrefetchWindowAndResetClearsThem() throws {
        var store = SemanticStore()
        let modelID = ModelId(3)
        try store.createModel(id: modelID, modelType: .table, itemCount: 1_000)
        let model = try #require(store.getModel(modelID))
        var tracker = CollectionRangeTracker()
        let first = tracker.requests(
            visibleStart: 0,
            visibleCount: 8,
            itemCount: 1_000,
            model: model,
            nodeID: NodeId(1),
            modelID: modelID
        )
        #expect(first.count == 1)
        #expect(first[0].startIndex == 0)

        let afterScroll = tracker.requests(
            visibleStart: 800,
            visibleCount: 8,
            itemCount: 1_000,
            model: model,
            nodeID: NodeId(1),
            modelID: modelID
        )
        #expect(afterScroll.count == 1)
        #expect(afterScroll[0].startIndex >= 768)

        tracker.reset()
        let afterReset = tracker.requests(
            visibleStart: 800,
            visibleCount: 8,
            itemCount: 1_000,
            model: model,
            nodeID: NodeId(1),
            modelID: modelID
        )
        #expect(afterReset.count == 1)
        #expect(afterReset[0].startIndex == afterScroll[0].startIndex)
    }
}
