//
// PendingEventReplayLoopTests.swift
// SRUITests
//
// Unit tests for lease-scoped replay scheduling and cancellation.
//

import Testing
import Foundation
@testable import Session

private actor LoopTestCounters {
    private(set) var replayCount = 0
    private(set) var failureCount = 0
    private(set) var finishCount = 0
    private(set) var failureMessage: String?

    @discardableResult
    func recordReplay() -> Int {
        replayCount += 1
        return replayCount
    }

    func recordFailure(_ message: String? = nil) {
        failureCount += 1
        failureMessage = message
    }

    func recordFinish() {
        finishCount += 1
    }
}

private actor LoopTestSignal {
    private var permits = 0
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        if permits > 0 {
            permits -= 1
            return
        }
        await withCheckedContinuation { continuation in
            waiters.append(continuation)
        }
    }

    func signal() {
        if waiters.isEmpty {
            permits += 1
        } else {
            waiters.removeFirst().resume()
        }
    }
}

private actor ReplaySleepProbe {
    private struct Waiter {
        let count: Int
        let continuation: CheckedContinuation<Void, Never>
    }

    private let releases: AsyncStream<Void>
    private let releaseContinuation: AsyncStream<Void>.Continuation
    private var recordedDurations: [Duration] = []
    private var waiters: [Waiter] = []

    init() {
        let (releases, releaseContinuation) = AsyncStream<Void>.makeStream()
        self.releases = releases
        self.releaseContinuation = releaseContinuation
    }

    func sleep(for duration: Duration) async throws {
        recordedDurations.append(duration)
        let ready = waiters.filter { $0.count <= recordedDurations.count }
        waiters.removeAll { $0.count <= recordedDurations.count }
        for waiter in ready {
            waiter.continuation.resume()
        }

        var iterator = releases.makeAsyncIterator()
        _ = await iterator.next()
        try Task.checkCancellation()
    }

    func waitForSleepCount(_ count: Int) async {
        if recordedDurations.count >= count { return }
        await withCheckedContinuation { continuation in
            waiters.append(Waiter(count: count, continuation: continuation))
        }
    }

    func releaseNextSleep() {
        releaseContinuation.yield()
    }

    var durations: [Duration] {
        recordedDurations
    }
}

private actor ReplayLoopHarness {
    private var loop: PendingEventReplayLoop

    init(
        initialDelay: Duration,
        maximumDelay: Duration,
        sleep: @escaping PendingEventReplayLoop.SleepOperation = { delay in
            try await Task<Never, Never>.sleep(for: delay)
        }
    ) {
        loop = PendingEventReplayLoop(
            initialDelay: initialDelay,
            maximumDelay: maximumDelay,
            sleep: sleep
        )
    }

    func start(
        resumeScope: UInt64,
        replay: @escaping PendingEventReplayLoop.ReplayOperation,
        onFailure: PendingEventReplayLoop.FailureHandler? = nil,
        onFinish: @escaping PendingEventReplayLoop.FinishHandler = { _ in }
    ) -> PendingEventReplayLoop.Lease {
        loop.start(
            resumeScope: resumeScope,
            replay: replay,
            onFailure: onFailure,
            onFinish: { [weak self] lease in
                await self?.finish(lease, then: onFinish)
            }
        )
    }

    func invalidate(_ lease: PendingEventReplayLoop.Lease) {
        loop.invalidate(lease)
    }

    func isActive(_ lease: PendingEventReplayLoop.Lease) -> Bool {
        loop.isActive(lease)
    }

    var isRunning: Bool {
        loop.isRunning
    }

    private func finish(
        _ lease: PendingEventReplayLoop.Lease,
        then onFinish: PendingEventReplayLoop.FinishHandler
    ) async {
        loop.finish(lease)
        await onFinish(lease)
    }
}

@Suite("PendingEventReplayLoop Tests")
struct PendingEventReplayLoopTests {

    @Test("Superseding start invalidates the prior lease")
    func supersedingStartInvalidatesPriorLease() async {
        let harness = ReplayLoopHarness(
            initialDelay: .seconds(3_600),
            maximumDelay: .seconds(3_600)
        )
        let firstLease = await harness.start(
            resumeScope: 1,
            replay: { _ in true }
        )
        #expect(await harness.isActive(firstLease))

        let secondLease = await harness.start(
            resumeScope: 2,
            replay: { _ in false }
        )

        #expect(await harness.isActive(secondLease))
        #expect(await harness.isActive(firstLease) == false)
        #expect(await harness.isRunning)
        await harness.invalidate(secondLease)
        #expect(await harness.isRunning == false)
    }

    @Test("Stale lease invalidate is a no-op")
    func staleLeaseInvalidateIsNoOp() async {
        let harness = ReplayLoopHarness(
            initialDelay: .seconds(3_600),
            maximumDelay: .seconds(3_600)
        )
        let firstLease = await harness.start(
            resumeScope: 1,
            replay: { _ in true }
        )
        let activeLease = await harness.start(
            resumeScope: 2,
            replay: { _ in true }
        )

        await harness.invalidate(firstLease)
        #expect(await harness.isActive(activeLease))
        #expect(await harness.isRunning)

        await harness.invalidate(activeLease)
        #expect(await harness.isRunning == false)
    }

    @Test("Replay returning false finishes the lease without calling onFailure")
    func replayReturningFalseFinishesLease() async {
        let counters = LoopTestCounters()
        let finished = LoopTestSignal()
        let harness = ReplayLoopHarness(initialDelay: .zero, maximumDelay: .zero)

        _ = await harness.start(
            resumeScope: 1,
            replay: { _ in
                await counters.recordReplay()
                return false
            },
            onFailure: { message in
                await counters.recordFailure(message)
            },
            onFinish: { _ in
                await counters.recordFinish()
                await finished.signal()
            }
        )

        await finished.wait()
        #expect(await counters.replayCount == 1)
        #expect(await counters.failureCount == 0)
        #expect(await counters.finishCount == 1)
        #expect(await harness.isRunning == false)
    }

    @Test("External invalidate clears running state before onFinish runs")
    func externalInvalidateClearsRunningState() async {
        let counters = LoopTestCounters()
        let finished = LoopTestSignal()
        let harness = ReplayLoopHarness(
            initialDelay: .seconds(3_600),
            maximumDelay: .seconds(3_600)
        )
        let lease = await harness.start(
            resumeScope: 1,
            replay: { _ in true },
            onFinish: { _ in
                await counters.recordFinish()
                await finished.signal()
            }
        )

        #expect(await harness.isRunning)
        await harness.invalidate(lease)
        #expect(await harness.isRunning == false)

        await finished.wait()
        #expect(await counters.finishCount == 1)
    }

    @Test("Replay error invokes onFailure once and finishes the lease")
    func replayErrorInvokesOnFailureOnce() async {
        struct ReplayFailure: Error {}
        let counters = LoopTestCounters()
        let finished = LoopTestSignal()
        let harness = ReplayLoopHarness(initialDelay: .zero, maximumDelay: .zero)

        _ = await harness.start(
            resumeScope: 1,
            replay: { _ in throw ReplayFailure() },
            onFailure: { message in
                await counters.recordFailure(message)
            },
            onFinish: { _ in
                await counters.recordFinish()
                await finished.signal()
            }
        )

        await finished.wait()
        #expect(await counters.failureCount == 1)
        #expect(await counters.failureMessage != nil)
        #expect(await counters.finishCount == 1)
        #expect(await harness.isRunning == false)
    }

    @Test("Retry delay doubles and caps at the configured maximum")
    func retryDelayBackoffCapsAtMaximum() async {
        let counters = LoopTestCounters()
        let finished = LoopTestSignal()
        let sleeper = ReplaySleepProbe()
        let harness = ReplayLoopHarness(
            initialDelay: .seconds(1),
            maximumDelay: .seconds(4),
            sleep: { duration in
                try await sleeper.sleep(for: duration)
            }
        )

        _ = await harness.start(
            resumeScope: 1,
            replay: { _ in
                let replayCount = await counters.recordReplay()
                return replayCount < 3
            },
            onFinish: { _ in
                await counters.recordFinish()
                await finished.signal()
            }
        )

        await sleeper.waitForSleepCount(1)
        #expect(await sleeper.durations == [.seconds(1)])
        await sleeper.releaseNextSleep()

        await sleeper.waitForSleepCount(2)
        #expect(await sleeper.durations == [.seconds(1), .seconds(2)])
        await sleeper.releaseNextSleep()

        await sleeper.waitForSleepCount(3)
        #expect(await sleeper.durations == [.seconds(1), .seconds(2), .seconds(4)])
        await sleeper.releaseNextSleep()

        await finished.wait()
        #expect(await counters.replayCount == 3)
        #expect(await counters.finishCount == 1)
        #expect(await harness.isRunning == false)
    }
}
