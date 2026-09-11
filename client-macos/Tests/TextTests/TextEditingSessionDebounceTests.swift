//
// TextEditingSessionDebounceTests.swift
// TextTests
//
// Deterministic debounce scheduling and cancellation coverage.
//

import SemanticModel
import Testing
@testable import Text

private actor ManualDebounceSleeper {
    struct Snapshot: Equatable, Sendable {
        let requestedDelays: [UInt64]
        let pendingCount: Int
        let cancellationCount: Int
        let releaseCount: Int
    }

    private struct PendingSleep {
        let continuation: CheckedContinuation<Void, any Error>
    }

    private var nextID: UInt64 = 1
    private var requestedDelays: [UInt64] = []
    private var pendingOrder: [UInt64] = []
    private var pending: [UInt64: PendingSleep] = [:]
    private var cancellationCount = 0
    private var releaseCount = 0

    func sleep(nanoseconds: UInt64) async throws {
        let requestID = nextID
        nextID += 1
        try Task.checkCancellation()

        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                requestedDelays.append(nanoseconds)
                pendingOrder.append(requestID)
                pending[requestID] = PendingSleep(continuation: continuation)
            }
        } onCancel: {
            Task {
                await self.cancel(requestID)
            }
        }
    }

    func releaseNext() -> Bool {
        guard let requestID = pendingOrder.first,
              let sleep = pending.removeValue(forKey: requestID) else {
            return false
        }
        pendingOrder.removeFirst()
        releaseCount += 1
        sleep.continuation.resume()
        return true
    }

    func releaseAll() -> Int {
        var released = 0
        while let requestID = pendingOrder.first {
            pendingOrder.removeFirst()
            guard let sleep = pending.removeValue(forKey: requestID) else {
                continue
            }
            releaseCount += 1
            released += 1
            sleep.continuation.resume()
        }
        return released
    }

    func snapshot() -> Snapshot {
        Snapshot(
            requestedDelays: requestedDelays,
            pendingCount: pending.count,
            cancellationCount: cancellationCount,
            releaseCount: releaseCount
        )
    }

    private func cancel(_ requestID: UInt64) {
        guard let sleep = pending.removeValue(forKey: requestID) else { return }
        pendingOrder.removeAll { $0 == requestID }
        cancellationCount += 1
        sleep.continuation.resume(throwing: CancellationError())
    }
}

private struct RecordedCommit: Equatable {
    let text: String
    let editSeq: UInt64
}

@MainActor
private final class CommitRecorder {
    private(set) var commits: [RecordedCommit] = []

    func attach(to session: TextEditingSession) {
        session.onCommit = { [weak self] _, text, editSeq, _ in
            self?.commits.append(RecordedCommit(text: text, editSeq: editSeq.rawValue))
        }
    }
}

@Suite("TextEditingSession deterministic debounce")
@MainActor
struct TextEditingSessionDebounceTests {
    private let nodeID = NodeId(12)
    private let delay: UInt64 = 75_000_000

    @Test("A pending debounce does not commit before its deadline")
    func noEarlyCommit() async {
        let (session, sleeper, recorder) = makeSession()

        session.noteLocalValue(
            "draft",
            nodeID: nodeID,
            composing: false,
            flushImmediately: false
        )

        let snapshot = await waitForState(sleeper, requestCount: 1, pendingCount: 1)
        #expect(snapshot.requestedDelays == [delay])
        #expect(recorder.commits.isEmpty)

        session.resetForReplacementSession()
        let canceled = await waitForState(sleeper, requestCount: 1, pendingCount: 0)
        #expect(canceled.cancellationCount == 1)
    }

    @Test("Releasing the debounce deadline commits exactly once")
    func deadlineCommitsExactlyOnce() async {
        let (session, sleeper, recorder) = makeSession()

        session.noteLocalValue(
            "ready",
            nodeID: nodeID,
            composing: false,
            flushImmediately: false
        )
        _ = await waitForState(sleeper, requestCount: 1, pendingCount: 1)

        #expect(await sleeper.releaseNext())
        await waitForCommitCount(1, recorder: recorder)

        #expect(recorder.commits == [RecordedCommit(text: "ready", editSeq: 1)])
        #expect(session.nextEditSeqValue(for: nodeID) == 2)
        let snapshot = await sleeper.snapshot()
        #expect(snapshot.pendingCount == 0)
        #expect(snapshot.releaseCount == 1)
    }

    @Test("New input cancels and restarts the debounce with the newest value")
    func newerInputRestartsDebounce() async {
        let (session, sleeper, recorder) = makeSession()

        session.noteLocalValue("a", nodeID: nodeID, composing: false, flushImmediately: false)
        _ = await waitForState(sleeper, requestCount: 1, pendingCount: 1)

        session.noteLocalValue("ab", nodeID: nodeID, composing: false, flushImmediately: false)
        let restarted = await waitForState(sleeper, requestCount: 2, pendingCount: 1)
        #expect(restarted.requestedDelays == [delay, delay])
        #expect(restarted.cancellationCount == 1)
        #expect(recorder.commits.isEmpty)

        #expect(await sleeper.releaseNext())
        await waitForCommitCount(1, recorder: recorder)

        #expect(recorder.commits == [RecordedCommit(text: "ab", editSeq: 1)])
        #expect(session.nextEditSeqValue(for: nodeID) == 2)
    }

    @Test("End editing flushes immediately and a canceled deadline cannot duplicate it")
    func endEditingFlushesWithoutDuplicate() async {
        let (session, sleeper, recorder) = makeSession()

        session.noteLocalValue("done", nodeID: nodeID, composing: false, flushImmediately: false)
        _ = await waitForState(sleeper, requestCount: 1, pendingCount: 1)

        session.endEditing(nodeID: nodeID)
        #expect(recorder.commits == [RecordedCommit(text: "done", editSeq: 1)])

        let canceled = await waitForState(sleeper, requestCount: 1, pendingCount: 0)
        #expect(canceled.cancellationCount == 1)
        #expect(await sleeper.releaseAll() == 0)
        await settleScheduledTasks()
        #expect(recorder.commits == [RecordedCommit(text: "done", editSeq: 1)])
    }

    @Test("Flush all commits immediately and a canceled deadline cannot duplicate it")
    func flushAllPendingFlushesWithoutDuplicate() async {
        let (session, sleeper, recorder) = makeSession()

        session.noteLocalValue("submit", nodeID: nodeID, composing: false, flushImmediately: false)
        _ = await waitForState(sleeper, requestCount: 1, pendingCount: 1)

        session.flushAllPending()
        #expect(recorder.commits == [RecordedCommit(text: "submit", editSeq: 1)])

        let canceled = await waitForState(sleeper, requestCount: 1, pendingCount: 0)
        #expect(canceled.cancellationCount == 1)
        #expect(await sleeper.releaseAll() == 0)
        await settleScheduledTasks()
        #expect(recorder.commits == [RecordedCommit(text: "submit", editSeq: 1)])
    }

    @Test("Replacement reset cancels a pending deadline without committing")
    func replacementResetCancelsPendingDeadline() async {
        let (session, sleeper, recorder) = makeSession()

        session.noteLocalValue("old", nodeID: nodeID, composing: false, flushImmediately: false)
        _ = await waitForState(sleeper, requestCount: 1, pendingCount: 1)

        session.resetForReplacementSession()

        let canceled = await waitForState(sleeper, requestCount: 1, pendingCount: 0)
        #expect(canceled.cancellationCount == 1)
        #expect(await sleeper.releaseAll() == 0)
        await settleScheduledTasks()
        #expect(recorder.commits.isEmpty)
        #expect(session.localValue(for: nodeID) == nil)
        #expect(session.nextEditSeqValue(for: nodeID) == 1)
    }

    @Test("Deleting a node cancels its pending deadline without committing")
    func nodeDeletionCancelsPendingDeadline() async {
        let (session, sleeper, recorder) = makeSession()

        session.noteLocalValue("deleted", nodeID: nodeID, composing: false, flushImmediately: false)
        _ = await waitForState(sleeper, requestCount: 1, pendingCount: 1)

        session.syncPresentNodes([])

        let canceled = await waitForState(sleeper, requestCount: 1, pendingCount: 0)
        #expect(canceled.cancellationCount == 1)
        #expect(await sleeper.releaseAll() == 0)
        await settleScheduledTasks()
        #expect(recorder.commits.isEmpty)
        #expect(session.localValue(for: nodeID) == nil)
        #expect(session.nextEditSeqValue(for: nodeID) == 1)
    }

    @Test("Marked text does not start debounce; committed composition does")
    func compositionStartsDebounceOnlyForCommittedText() async {
        let (session, sleeper, recorder) = makeSession()

        _ = session.setComposing(true, nodeID: nodeID)
        session.noteLocalValue(
            "marked",
            nodeID: nodeID,
            composing: true,
            flushImmediately: false
        )

        var snapshot = await sleeper.snapshot()
        #expect(snapshot.requestedDelays.isEmpty)
        #expect(snapshot.pendingCount == 0)
        #expect(recorder.commits.isEmpty)

        #expect(session.setComposing(false, nodeID: nodeID) == nil)
        #expect(recorder.commits.isEmpty)

        session.noteLocalValue(
            "committed",
            nodeID: nodeID,
            composing: false,
            flushImmediately: false
        )
        snapshot = await waitForState(sleeper, requestCount: 1, pendingCount: 1)
        #expect(snapshot.requestedDelays == [delay])

        #expect(await sleeper.releaseNext())
        await waitForCommitCount(1, recorder: recorder)
        #expect(recorder.commits == [RecordedCommit(text: "committed", editSeq: 1)])
    }

    private func makeSession() -> (
        session: TextEditingSession,
        sleeper: ManualDebounceSleeper,
        recorder: CommitRecorder
    ) {
        let sleeper = ManualDebounceSleeper()
        let session = TextEditingSession(
            debounceNanoseconds: delay,
            sleep: { duration in
                try await sleeper.sleep(nanoseconds: duration)
            }
        )
        let recorder = CommitRecorder()
        recorder.attach(to: session)
        return (session, sleeper, recorder)
    }

    private func waitForState(
        _ sleeper: ManualDebounceSleeper,
        requestCount: Int,
        pendingCount: Int
    ) async -> ManualDebounceSleeper.Snapshot {
        var snapshot = await sleeper.snapshot()
        for _ in 0..<100 {
            if snapshot.requestedDelays.count == requestCount,
               snapshot.pendingCount == pendingCount {
                return snapshot
            }
            await Task.yield()
            snapshot = await sleeper.snapshot()
        }
        return snapshot
    }

    private func waitForCommitCount(_ count: Int, recorder: CommitRecorder) async {
        for _ in 0..<100 where recorder.commits.count < count {
            await Task.yield()
        }
    }

    private func settleScheduledTasks() async {
        for _ in 0..<10 {
            await Task.yield()
        }
    }
}
