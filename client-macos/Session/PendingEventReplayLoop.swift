//
// PendingEventReplayLoop.swift
// Session
//
// Scheduling and lifecycle for delayed replay of retained semantic events.
//

import Foundation

/// Schedules delayed replay attempts without owning the pending events or their transport.
///
/// The owner supplies the replay operation and retains authority over event identity, ordering,
/// settlement, and wire serialization. A lease makes cancellation generation-specific, so stale
/// resume work cannot invalidate a newer replay loop.
struct PendingEventReplayLoop {
    static let defaultInitialDelay: Duration = .seconds(1)
    static let defaultMaximumDelay: Duration = .seconds(30)

    struct Lease: Equatable, Sendable {
        /// Resume generation that owns this lease; strictly increasing per outbox (§18).
        let resumeScope: UInt64
        fileprivate let token: UUID
    }

    /// Returns `true` when another delayed replay should be scheduled; `false` stops the loop.
    typealias ReplayOperation = @Sendable (Lease) async throws -> Bool
    typealias FailureHandler = @Sendable (String) async -> Void
    typealias FinishHandler = @Sendable (Lease) async -> Void
    typealias SleepOperation = @Sendable (Duration) async throws -> Void

    private let initialDelay: Duration
    private let maximumDelay: Duration
    private let sleep: SleepOperation
    private var activeLease: Lease?
    private var task: Task<Void, Never>?

    init(
        initialDelay: Duration = PendingEventReplayLoop.defaultInitialDelay,
        maximumDelay: Duration = PendingEventReplayLoop.defaultMaximumDelay,
        sleep: @escaping SleepOperation = { delay in
            try await Task<Never, Never>.sleep(for: delay)
        }
    ) {
        let clampedInitialDelay = Swift.max(.zero, initialDelay)
        self.initialDelay = clampedInitialDelay
        self.maximumDelay = Swift.max(clampedInitialDelay, maximumDelay)
        self.sleep = sleep
    }

    var isRunning: Bool {
        task != nil
    }

    func isActive(_ lease: Lease) -> Bool {
        activeLease == lease
    }

    @discardableResult
    mutating func start(
        resumeScope: UInt64,
        replay: @escaping ReplayOperation,
        onFailure: FailureHandler?,
        onFinish: @escaping FinishHandler
    ) -> Lease {
        invalidate()

        let lease = Lease(resumeScope: resumeScope, token: UUID())
        let initialDelay = self.initialDelay
        let maximumDelay = self.maximumDelay
        let sleep = self.sleep
        activeLease = lease
        task = Task {
            await Self.run(
                lease: lease,
                initialDelay: initialDelay,
                maximumDelay: maximumDelay,
                sleep: sleep,
                replay: replay,
                onFailure: onFailure
            )
            await onFinish(lease)
        }
        return lease
    }

    mutating func finish(_ lease: Lease) {
        guard activeLease == lease else { return }
        task = nil
        activeLease = nil
    }

    mutating func invalidate(_ lease: Lease) {
        guard activeLease == lease else { return }
        invalidate()
    }

    mutating func invalidate() {
        let task = task
        self.task = nil
        activeLease = nil
        task?.cancel()
    }

    private static func run(
        lease: Lease,
        initialDelay: Duration,
        maximumDelay: Duration,
        sleep: @escaping SleepOperation,
        replay: @escaping ReplayOperation,
        onFailure: FailureHandler?
    ) async {
        var retryDelay = initialDelay

        while Task.isCancelled == false {
            do {
                try await sleep(retryDelay)
            } catch {
                return
            }

            guard Task.isCancelled == false else { return }

            do {
                guard try await replay(lease) else { return }
            } catch is CancellationError {
                return
            } catch {
                guard Task.isCancelled == false else { return }
                await onFailure?(String(describing: error))
                return
            }

            retryDelay = Swift.min(retryDelay * 2, maximumDelay)
        }
    }
}
